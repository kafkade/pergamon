// SPDX-License-Identifier: AGPL-3.0-only

//! Shared state for the OPAQUE auth control plane.
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! Holds the auth store, the OPRF server secret ([`ServerSetup`], loaded from
//! outside the verifier DB), the per-identity throttle policy, and a short-lived
//! in-memory map of pending logins (the server-side [`ServerLogin`] state that
//! bridges the KE1→KE2 start step and the KE3 finish step).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use opaque_ke::{ServerLogin, ServerSetup};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::PergamonCipherSuite;
use crate::auth::store::AuthStore;
use crate::auth::throttle::ThrottleConfig;
use crate::error::ApiError;

/// How long a pending login may sit between the start and finish steps.
const PENDING_LOGIN_TTL_MS: i64 = 120_000;

/// Hard cap on concurrently pending logins, to bound memory for this
/// process-wide map. Per-IP admission control that makes this cap hard to reach
/// under abuse is WP-4/#195.
const MAX_PENDING_LOGINS: usize = 10_000;

/// Current epoch time in milliseconds.
fn now_ms() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

/// One in-flight login awaiting its finish (KE3) message.
struct PendingLogin {
    /// Server-side login state produced by `ServerLogin::start`.
    state: ServerLogin<PergamonCipherSuite>,
    /// The identity this login is for (to key throttling on finish).
    identity_handle: String,
    /// Epoch millis after which this pending login is discarded.
    expires_at_ms: i64,
}

/// Shared state for the OPAQUE auth endpoints. Cheaply cloneable (all fields are
/// shared handles), as required by axum's `State`.
#[derive(Clone)]
pub struct AuthState {
    /// The verifier / handle-map / throttle store (separate DB).
    store: Arc<Mutex<AuthStore>>,
    /// The OPRF server secret. Loaded from outside the verifier DB (design
    /// §1.8) and never persisted alongside the `accounts` rows it protects.
    server_setup: Arc<ServerSetup<PergamonCipherSuite>>,
    /// Identifier of the OPRF key accounts are registered under (design §1.8,
    /// for future rotation).
    oprf_key_id: String,
    /// Per-identity throttle policy.
    throttle: ThrottleConfig,
    /// Short-lived server-side login states, keyed by `login_id`.
    ///
    /// **Single-instance / non-persistent seam — WP-3e ([#201]).** This map
    /// lives only in this process's memory. It is therefore *not* shared across
    /// replicas and does *not* survive a restart: a login whose `login/start`
    /// landed on one instance cannot be finished on another, and a restart
    /// between start and finish drops the pending state (the client simply
    /// retries the login — no security impact, only a retriable failure). A
    /// horizontally-scaled deployment must replace this with sticky routing or a
    /// shared/pooled store for pending logins; that is explicitly deferred to
    /// WP-3e multi-instance work.
    ///
    /// [#201]: https://github.com/kafkade/pergamon/issues/201
    pending: Arc<Mutex<HashMap<String, PendingLogin>>>,
}

impl AuthState {
    /// Assemble auth state from its parts.
    #[must_use]
    pub fn new(
        store: AuthStore,
        server_setup: ServerSetup<PergamonCipherSuite>,
        oprf_key_id: impl Into<String>,
        throttle: ThrottleConfig,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            server_setup: Arc::new(server_setup),
            oprf_key_id: oprf_key_id.into(),
            throttle,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Lock the auth store, mapping a poisoned lock to a 500.
    ///
    /// # Errors
    /// Returns [`ApiError::internal`] if the mutex is poisoned.
    pub fn lock_store(&self) -> Result<MutexGuard<'_, AuthStore>, ApiError> {
        self.store
            .lock()
            .map_err(|_| ApiError::internal("auth store lock poisoned"))
    }

    /// The OPRF server secret.
    #[must_use]
    pub fn server_setup(&self) -> &ServerSetup<PergamonCipherSuite> {
        &self.server_setup
    }

    /// The OPRF key id new registrations are stamped with.
    #[must_use]
    pub fn oprf_key_id(&self) -> &str {
        &self.oprf_key_id
    }

    /// The per-identity throttle policy.
    #[must_use]
    pub const fn throttle(&self) -> &ThrottleConfig {
        &self.throttle
    }

    /// Store a pending login's server state, returning a fresh `login_id`.
    ///
    /// Expired entries are pruned first; if the map is still at capacity the
    /// insert is rejected with a 503-style error (WP-4/#195 owns the per-IP
    /// admission control that keeps this from being reachable under abuse).
    ///
    /// # Errors
    /// Returns [`ApiError::internal`] on a poisoned lock, or
    /// [`ApiError`] (503) when the pending-login table is saturated.
    pub fn insert_pending(
        &self,
        state: ServerLogin<PergamonCipherSuite>,
        identity_handle: &str,
    ) -> Result<String, ApiError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| ApiError::internal("pending-login lock poisoned"))?;
        let now = now_ms();
        pending.retain(|_, p| p.expires_at_ms > now);
        if pending.len() >= MAX_PENDING_LOGINS {
            return Err(ApiError::unavailable(
                "too many pending logins; retry later",
            ));
        }
        let login_id = Uuid::new_v4().simple().to_string();
        pending.insert(
            login_id.clone(),
            PendingLogin {
                state,
                identity_handle: identity_handle.to_string(),
                expires_at_ms: now + PENDING_LOGIN_TTL_MS,
            },
        );
        drop(pending);
        Ok(login_id)
    }

    /// Remove and return a pending login's server state and identity, if it
    /// exists and has not expired.
    ///
    /// # Errors
    /// Returns [`ApiError::internal`] on a poisoned lock.
    pub fn take_pending(
        &self,
        login_id: &str,
    ) -> Result<Option<(ServerLogin<PergamonCipherSuite>, String)>, ApiError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| ApiError::internal("pending-login lock poisoned"))?;
        let now = now_ms();
        match pending.remove(login_id) {
            Some(p) if p.expires_at_ms > now => Ok(Some((p.state, p.identity_handle))),
            _ => Ok(None),
        }
    }
}
