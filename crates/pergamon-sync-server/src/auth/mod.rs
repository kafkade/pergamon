// SPDX-License-Identifier: AGPL-3.0-only

//! OPAQUE server-auth identity for hosted (multi-tenant) sync — WP-3a, #189.
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! This module implements the server half of the OPAQUE aPAKE from ADR-029 and
//! the security design in `docs/design/hosted-auth-control-plane.md`. Per that
//! design's external-review gate (§1.11), **this code MUST NOT ship to
//! production until an independent security reviewer has signed off** the
//! chosen crate version, cipher suite, KSF parameters, no-existence-oracle
//! property, OPRF-key handling, and throttling against the exact integration we
//! intend to deploy. It is a reviewable first implementation, not a
//! production-ready one.
//!
//! ## What this is
//! - A **separate** auth store ([`store::AuthStore`]) with its own `SQLite`
//!   connection, holding the OPAQUE registration record (a **verifier only**,
//!   never a password), the internal `identity_handle → account_id` map, and
//!   per-identity throttling counters. It is never co-mingled with the blind
//!   relay's content tables in [`crate::store::SyncStore`].
//! - OPAQUE registration + login endpoints ([`routes`]) that are mounted **only
//!   in multi-tenant mode** (see [`ServerMode`]); blind mode is unchanged.
//! - Privacy-preserving lookup: an unknown identity is served the library's
//!   dummy-login path, indistinguishable from a wrong-password attempt, so there
//!   is no account-existence oracle for unauthenticated callers.
//!
//! ## Auth plane ⟂ content plane
//! A successful OPAQUE login proves identity to the operator (for quotas /
//! billing) but **never** yields the ARK or any content key (ADR-024/ADR-029).
//! This module never touches content keys; it only reads/writes verifiers,
//! handles, and throttling state.
//!
//! ## Out of scope (seams left for later WPs)
//! - Per-IP rate limiting, body caps, storage-DoS isolation — WP-4/#195.
//! - Per-route authorization + tenant isolation — WP-3c/#197, implemented in
//!   [`authz`]. This module mints/refreshes/revokes device tokens (WP-3b/#192)
//!   and exposes [`store::AuthStore::validate_token`] as the primitive;
//!   [`authz::require_account_auth`] consumes it to gate the blind content
//!   routes in the multi-tenant router builders.

pub mod authz;
pub mod cipher_suite;
pub mod routes;
pub mod state;
pub mod store;
pub mod throttle;
pub mod token;
pub mod wire;

pub use authz::{authorize_account, require_account_auth};
pub use cipher_suite::PergamonCipherSuite;
pub use state::AuthState;
pub use token::{AuthAccount, TokenConfig};

use axum::Router;
use axum::routing::post;

/// Build the OPAQUE auth sub-router (mounted only in multi-tenant mode).
///
/// Returns a fully-stated `Router` that can be merged into the content router.
/// Covers OPAQUE register/login (WP-3a, #189) plus per-device token
/// refresh/revocation (WP-3b, #192). Token **issuance** is folded into
/// `login/finish`, so it needs no separate route.
pub fn auth_router(auth_state: AuthState) -> Router {
    Router::new()
        .route("/v1/auth/register/start", post(routes::register_start))
        .route("/v1/auth/register/finish", post(routes::register_finish))
        .route("/v1/auth/login/start", post(routes::login_start))
        .route("/v1/auth/login/finish", post(routes::login_finish))
        .route("/v1/auth/token/refresh", post(routes::token_refresh))
        .route("/v1/auth/token/revoke", post(routes::token_revoke))
        .with_state(auth_state)
}

/// Deployment mode for the sync server (design §Part 4, Q3).
///
/// Defaults to [`ServerMode::Blind`] so an existing self-hoster's behavior is
/// unchanged (ADR-026): the OPAQUE auth routes are only mounted in
/// [`ServerMode::Multitenant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerMode {
    /// Single-account blind relay — no auth plane (today's ADR-026 behavior).
    #[default]
    Blind,
    /// Multi-tenant hosting — mounts the OPAQUE auth control plane.
    Multitenant,
}

impl ServerMode {
    /// Parse a mode string (e.g. from `PERGAMON_SYNC_MODE`), case-insensitively.
    ///
    /// Unknown values map to [`ServerMode::Blind`] (the safe default) and the
    /// caller is expected to warn.
    #[must_use]
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "blind" => Some(Self::Blind),
            "multitenant" | "multi-tenant" => Some(Self::Multitenant),
            _ => None,
        }
    }
}

#[cfg(test)]
mod cipher_suite_tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]

    use argon2::Argon2;
    use opaque_ke::{
        ClientLogin, ClientLoginFinishParameters, ClientRegistration,
        ClientRegistrationFinishParameters, ServerLogin, ServerLoginParameters, ServerRegistration,
        ServerSetup,
    };
    use rand::rngs::OsRng;

    use super::PergamonCipherSuite;

    /// Drive a full OPAQUE register → login round-trip through the pinned cipher
    /// suite to prove the exact `opaque-ke 4.0.1` API and our type parameters
    /// compile and interoperate. Uses fast Argon2 params so the test is quick.
    #[test]
    fn opaque_round_trip_succeeds() {
        // Fast KSF params so the test does not spend seconds in Argon2.
        let fast_ksf = Argon2::default();
        let mut rng = OsRng;
        let password = b"correct horse battery staple";
        let identity = b"alice";

        let server_setup = ServerSetup::<PergamonCipherSuite>::new(&mut rng);

        // Registration.
        let c_start = ClientRegistration::<PergamonCipherSuite>::start(&mut rng, password).unwrap();
        let s_start = ServerRegistration::<PergamonCipherSuite>::start(
            &server_setup,
            c_start.message,
            identity,
        )
        .unwrap();
        let c_finish = c_start
            .state
            .finish(
                &mut rng,
                password,
                s_start.message,
                ClientRegistrationFinishParameters {
                    ksf: Some(&fast_ksf),
                    ..Default::default()
                },
            )
            .unwrap();
        let password_file = ServerRegistration::<PergamonCipherSuite>::finish(c_finish.message);
        let record = password_file.serialize().to_vec();
        assert!(!record.is_empty());

        // Login.
        let c_login = ClientLogin::<PergamonCipherSuite>::start(&mut rng, password).unwrap();
        let stored = ServerRegistration::<PergamonCipherSuite>::deserialize(&record).unwrap();
        let s_login = ServerLogin::start(
            &mut rng,
            &server_setup,
            Some(stored),
            c_login.message,
            identity,
            ServerLoginParameters::default(),
        )
        .unwrap();
        let c_login_finish = c_login
            .state
            .finish(
                &mut rng,
                password,
                s_login.message,
                ClientLoginFinishParameters {
                    ksf: Some(&fast_ksf),
                    ..Default::default()
                },
            )
            .unwrap();
        let s_login_finish = s_login
            .state
            .finish(c_login_finish.message, ServerLoginParameters::default())
            .unwrap();

        assert_eq!(c_login_finish.session_key, s_login_finish.session_key);
    }

    /// The dummy-login path (unknown identity, `None` password file) must
    /// produce a `CredentialResponse` and then fail finalization — never a
    /// distinct "no such account" outcome.
    #[test]
    fn opaque_unknown_identity_uses_dummy_path() {
        let fast_ksf = Argon2::default();
        let mut rng = OsRng;
        let server_setup = ServerSetup::<PergamonCipherSuite>::new(&mut rng);

        let c_login = ClientLogin::<PergamonCipherSuite>::start(&mut rng, b"whatever").unwrap();
        // No stored record for this identity: pass `None`.
        let s_login = ServerLogin::start(
            &mut rng,
            &server_setup,
            None,
            c_login.message,
            b"ghost",
            ServerLoginParameters::default(),
        )
        .unwrap();
        // The client finalize fails (as with a wrong password) — no oracle.
        let result = c_login.state.finish(
            &mut rng,
            b"whatever",
            s_login.message,
            ClientLoginFinishParameters {
                ksf: Some(&fast_ksf),
                ..Default::default()
            },
        );
        assert!(result.is_err());
    }
}
