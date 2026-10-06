// SPDX-License-Identifier: AGPL-3.0-only

//! Application state shared across all request handlers.

use std::sync::{Arc, RwLock};

use crate::error::ApiError;
use crate::fairness::{FairnessConfig, TenantLimiter};
use crate::store::{StoreError, SyncStore};

/// Shared application state available to all request handlers.
///
/// ## Concurrency (WP-3e, #201)
/// The store is **not** wrapped in a process-wide mutex any more. [`SyncStore`]
/// owns one writer connection and a bounded pool of reader connections
/// internally, so concurrent tenants no longer serialize behind a single lock.
///
/// Store calls are blocking `SQLite` work, so handlers must never run them
/// inline on a Tokio worker thread — with a reader pool larger than the worker
/// count, a handful of slow reads would occupy every worker and one heavy tenant
/// could starve the whole runtime. [`AppState::with_store`] and
/// [`AppState::with_tenant_store`] are the only sanctioned way in: they move the
/// work onto `tokio::task::spawn_blocking`, whose pool exists for exactly this.
///
/// One closure is one connection checkout, so a handler that issues two reads
/// keeps them on the same connection.
#[derive(Clone)]
pub struct AppState {
    /// The encrypted event-log and blob store.
    pub store: Arc<SyncStore>,
    /// Per-tenant in-flight concurrency cap (WP-3e, #201), so one heavy tenant
    /// cannot hold every pooled connection.
    pub tenants: Arc<TenantLimiter>,
    pub(crate) binding_gate: Arc<RwLock<()>>,
    auth: Option<crate::auth::AuthState>,
}

impl AppState {
    /// Wrap a [`SyncStore`] in shared state, deriving the default per-tenant cap
    /// from the store's reader-pool size.
    #[must_use]
    pub fn new(store: SyncStore) -> Self {
        let fairness = FairnessConfig::for_pool(store.read_pool_size(), store.checkout_timeout());
        Self::with_fairness(store, fairness)
    }

    /// Wrap a [`SyncStore`] in shared state with an explicit per-tenant policy.
    #[must_use]
    pub fn with_fairness(store: SyncStore, fairness: FairnessConfig) -> Self {
        Self {
            store: Arc::new(store),
            tenants: Arc::new(TenantLimiter::new(fairness)),
            binding_gate: Arc::new(RwLock::new(())),
            auth: None,
        }
    }

    pub(crate) fn authenticated(mut self, auth: crate::auth::AuthState) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Keep an admitted namespace lease through the store operation. Bind cannot
    /// commit between admission and a queued write; read leases remain concurrent.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub async fn with_account_store<T, F>(
        &self,
        account_id: &str,
        principal: Option<crate::auth::AuthAccount>,
        op: F,
    ) -> Result<T, ApiError>
    where
        F: FnOnce(&SyncStore) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let state = self.clone();
        let account_id = account_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _lease = state
                .binding_gate
                .read()
                .map_err(|_| ApiError::internal("binding gate poisoned"))?;
            if let Some(auth) = &state.auth {
                let who = principal
                    .as_ref()
                    .ok_or_else(|| ApiError::unauthorized("missing principal"))?;
                crate::auth::authorize_account(who, &account_id, "STORE", "content")?;
                if !auth.lock_store()?.principal_is_current(who)? {
                    return Err(ApiError::unauthorized("invalid or expired token"));
                }
            }
            let tenant = principal
                .as_ref()
                .map_or(account_id.as_str(), |p| p.auth_tenant_id.as_str());
            let _slot = state.tenants.acquire(tenant)?;
            op(&state.store).map_err(ApiError::from)
        })
        .await
        .map_err(|e| {
            tracing::error!(error=%e, "authorized store task failed");
            ApiError::internal("internal storage error")
        })?
    }

    /// Run a blocking store operation off the async runtime.
    ///
    /// Use this for operations with no tenant of their own. Anything scoped to
    /// an `account_id` should use [`Self::with_tenant_store`] so it is covered by
    /// the per-tenant fairness cap.
    ///
    /// # Errors
    /// Propagates the closure's [`StoreError`] mapped to an [`ApiError`], or a
    /// 500 if the blocking task itself panicked.
    pub async fn with_store<T, F>(&self, op: F) -> Result<T, ApiError>
    where
        F: FnOnce(&SyncStore) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let store = Arc::clone(&self.store);
        match tokio::task::spawn_blocking(move || op(&store)).await {
            Ok(result) => result.map_err(ApiError::from),
            Err(e) => {
                tracing::error!(error = %e, "store task failed");
                Err(ApiError::internal("internal storage error"))
            }
        }
    }

    /// Run a blocking store operation for `account_id`, subject to the
    /// per-tenant concurrency cap.
    ///
    /// The tenant slot is acquired **inside** the blocking task and released
    /// when the closure returns, so it covers exactly the window in which the
    /// tenant holds a database connection.
    ///
    /// # Errors
    /// Returns `503` if the tenant is over its concurrency allowance, propagates
    /// the closure's [`StoreError`], or returns a 500 if the blocking task
    /// panicked.
    pub async fn with_tenant_store<T, F>(&self, account_id: &str, op: F) -> Result<T, ApiError>
    where
        F: FnOnce(&SyncStore) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let store = Arc::clone(&self.store);
        let tenants = Arc::clone(&self.tenants);
        let account_id = account_id.to_owned();
        let joined = tokio::task::spawn_blocking(move || {
            let slot = tenants.acquire(&account_id)?;
            let result = op(&store);
            drop(slot);
            result.map_err(ApiError::from)
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(error = %e, "store task failed");
                Err(ApiError::internal("internal storage error"))
            }
        }
    }
}

#[cfg(test)]
mod binding_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::auth::{AuthState, PergamonCipherSuite, store::AuthStore, throttle::ThrottleConfig};
    use opaque_ke::ServerSetup;
    use rand::rngs::OsRng;

    #[tokio::test]
    async fn admitted_old_principal_cannot_write_after_binding_commit() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let tenant = store
            .finish_registration("owner", b"fixture", "fixture")
            .unwrap();
        let public = [7_u8; 32];
        let pair = store
            .mint_pair(
                &tenant,
                "device",
                &public,
                crate::auth::TokenConfig::default(),
            )
            .unwrap();
        let old = store.validate_token(&pair.access_token).unwrap().unwrap();
        let auth = AuthState::new(
            store,
            ServerSetup::<PergamonCipherSuite>::new(&mut OsRng),
            "fixture",
            ThrottleConfig::default(),
        );
        let state = AppState::new(SyncStore::open_in_memory().unwrap()).authenticated(auth.clone());
        {
            let mut store = auth.lock_store().unwrap();
            let tx = store.connection_mut().transaction().unwrap();
            tx.execute(
                "UPDATE content_bindings SET state='retired' WHERE auth_tenant_id=?1",
                rusqlite::params![tenant],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO content_bindings
                (content_account_id,auth_tenant_id,state,binding_version)
                VALUES('canonical',?1,'active',1)",
                rusqlite::params![tenant],
            )
            .unwrap();
            tx.execute(
                "UPDATE tokens SET revoked_at=1 WHERE auth_tenant_id=?1",
                rusqlite::params![tenant],
            )
            .unwrap();
            tx.commit().unwrap();
            drop(store);
        }
        let target = tenant.clone();
        let result = state
            .with_account_store(&tenant, Some(old), move |store| {
                store.blob_put(&target, &crate::ct_hash(b"stale-write"), b"stale-write")
            })
            .await
            .unwrap_err();
        assert_eq!(result.status, axum::http::StatusCode::UNAUTHORIZED);
        assert!(
            state
                .store
                .blob_get(&tenant, &crate::ct_hash(b"stale-write"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn mismatched_database_pair_is_refused_without_replacing_the_marker() {
        let store = SyncStore::open_in_memory().unwrap();
        store.pair_auth_instance("first").unwrap();
        assert!(store.pair_auth_instance("different").is_err());
        assert!(store.pair_auth_instance("first").is_ok());
    }
}
