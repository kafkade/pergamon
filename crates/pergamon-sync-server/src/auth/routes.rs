// SPDX-License-Identifier: AGPL-3.0-only

//! OPAQUE registration and login endpoints (WP-3a, #189).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! These handlers implement the server side of the OPAQUE registration (2 round
//! trips) and login/AKE (3 messages) flows, plus privacy-preserving lookup and
//! per-identity throttling. They are mounted **only in multi-tenant mode**.
//!
//! ## Privacy (no account-existence oracle, design §1.6)
//! On login, an unknown identity is served the library's *dummy* path
//! ([`ServerLogin::start`] with `None`), and the KSF/OPRF work runs on that miss
//! path exactly as on a real record. A wrong password and an unknown identity
//! both fail at `login/finish` with the **same** uniform `401` body, so neither
//! the response shape nor the failure mode reveals whether an account exists.
//! (Timing-equivalence is an explicit external-review item — §1.6, §1.11.)
//!
//! ### No standalone account-lookup endpoint (deliberate)
//! There is **intentionally NO** separate, unauthenticated "does this account
//! exist / what is its `account_id`" endpoint in this module. The opaque
//! `account_id` is returned in exactly two places, both of which require proof
//! of control of the credential: to the party that just completed
//! **registration** (`register/finish`), and to a caller that has just
//! completed a **successful login** (`login/finish`). Folding "lookup" into the
//! authenticated result — rather than exposing it as a queryable endpoint — is
//! the deliberate privacy choice from ADR-029 / design §1.6: an unauthenticated
//! caller can never turn any route here into an account-existence oracle.
//!
//! ## Throttling (design §1.7)
//! Failures increment a per-identity counter keyed on the `identity_handle`
//! **uniformly** (existing or not); once past the threshold, `login/start`
//! returns a uniform `429`. This does not distinguish existence.
//!
//! ## Token issuance seam — WP-3b/#192
//! A successful login here establishes an authenticated session and returns the
//! account's opaque `account_id`. Minting the per-device bearer token bound to
//! the ADR-024 Ed25519 device key is WP-3b ([#192]); this is the seam for it.
//!
//! [#192]: https://github.com/kafkade/pergamon/issues/192

use axum::Json;
use axum::extract::State;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use opaque_ke::{
    CredentialFinalization, CredentialRequest, RegistrationRequest, RegistrationUpload,
    ServerLogin, ServerLoginParameters, ServerRegistration,
};
use rand::rngs::OsRng;

use crate::auth::PergamonCipherSuite;
use crate::auth::state::AuthState;
use crate::auth::wire::{
    LoginFinishRequest, LoginFinishResponse, LoginStartRequest, LoginStartResponse,
    RegisterFinishRequest, RegisterFinishResponse, RegisterStartRequest, RegisterStartResponse,
};
use crate::error::ApiError;

/// Decode an opaque base64 OPAQUE message, mapping failures to 400.
fn decode_b64(field: &str, value: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(value.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("invalid base64 {field}: {e}")))
}

/// Encode OPAQUE message bytes as standard base64.
fn encode_b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// `POST /v1/auth/register/start` — evaluate the OPRF and return the
/// registration response.
///
/// # Errors
/// Returns 400 on a malformed request; 500 on an internal failure.
pub async fn register_start(
    State(state): State<AuthState>,
    Json(req): Json<RegisterStartRequest>,
) -> Result<Json<RegisterStartResponse>, ApiError> {
    let request_bytes = decode_b64("registration_request_b64", &req.registration_request_b64)?;
    let request = RegistrationRequest::<PergamonCipherSuite>::deserialize(&request_bytes)
        .map_err(|_| ApiError::bad_request("malformed registration request"))?;

    let result = ServerRegistration::<PergamonCipherSuite>::start(
        state.server_setup(),
        request,
        req.identity_handle.as_bytes(),
    )
    .map_err(|_| ApiError::bad_request("registration start failed"))?;

    Ok(Json(RegisterStartResponse {
        registration_response_b64: encode_b64(&result.message.serialize()),
    }))
}

/// `POST /v1/auth/register/finish` — persist the verifier and allocate an
/// opaque `account_id`.
///
/// # Errors
/// Returns 400 on a malformed upload; 409 if the identity is already
/// registered; 500 on an internal failure.
pub async fn register_finish(
    State(state): State<AuthState>,
    Json(req): Json<RegisterFinishRequest>,
) -> Result<Json<RegisterFinishResponse>, ApiError> {
    let upload_bytes = decode_b64("registration_upload_b64", &req.registration_upload_b64)?;
    let upload = RegistrationUpload::<PergamonCipherSuite>::deserialize(&upload_bytes)
        .map_err(|_| ApiError::bad_request("malformed registration upload"))?;

    // The finalized record is the verifier/envelope — never the password.
    let record = ServerRegistration::<PergamonCipherSuite>::finish(upload);
    let record_bytes = record.serialize().to_vec();

    let oprf_key_id = state.oprf_key_id().to_string();
    let account_id = {
        let mut store = state.lock_store()?;
        store.finish_registration(&req.identity_handle, &record_bytes, &oprf_key_id)?
    };

    Ok(Json(RegisterFinishResponse { account_id }))
}

/// `POST /v1/auth/login/start` — begin the OPAQUE AKE (KE1 → KE2).
///
/// Applies the per-identity throttle first (uniform `429` when locked out), then
/// runs `ServerLogin::start` with the stored record or, for an unknown identity,
/// the dummy `None` path (design §1.6).
///
/// # Errors
/// Returns 400 on a malformed request; 429 when throttled; 500 on an internal
/// failure.
pub async fn login_start(
    State(state): State<AuthState>,
    Json(req): Json<LoginStartRequest>,
) -> Result<Json<LoginStartResponse>, ApiError> {
    // Throttle check (keyed uniformly on the handle — not an existence oracle).
    let locked = state
        .lock_store()?
        .locked_until(&req.identity_handle)?
        .is_some();
    if locked {
        return Err(ApiError::too_many_requests(
            "too many attempts; try again later",
        ));
    }

    let request_bytes = decode_b64("credential_request_b64", &req.credential_request_b64)?;
    let credential_request = CredentialRequest::<PergamonCipherSuite>::deserialize(&request_bytes)
        .map_err(|_| ApiError::bad_request("malformed credential request"))?;

    // Look up the verifier; `None` drives the indistinguishable dummy path. The
    // store lock is released before the (slower) OPAQUE work below.
    let record_bytes = state.lock_store()?.opaque_record(&req.identity_handle)?;
    let password_file = record_bytes
        .map(|bytes| ServerRegistration::<PergamonCipherSuite>::deserialize(&bytes))
        .transpose()
        .map_err(|_| ApiError::internal("stored verifier is corrupt"))?;

    let mut rng = OsRng;
    let result = ServerLogin::start(
        &mut rng,
        state.server_setup(),
        password_file,
        credential_request,
        req.identity_handle.as_bytes(),
        ServerLoginParameters::default(),
    )
    .map_err(|_| ApiError::bad_request("login start failed"))?;

    let login_id = state.insert_pending(result.state, &req.identity_handle)?;

    Ok(Json(LoginStartResponse {
        login_id,
        credential_response_b64: encode_b64(&result.message.serialize()),
    }))
}

/// `POST /v1/auth/login/finish` — complete the OPAQUE AKE (KE3).
///
/// On success, resets the failure counter and returns the authenticated
/// account's opaque `account_id`. On an invalid login (wrong password or the
/// dummy path for an unknown identity), records a failure and returns the
/// **uniform** `401`.
///
/// # Errors
/// Returns 400 on a malformed finalization; 401 on an invalid login or an
/// unknown/expired `login_id`; 429 when throttled; 500 on an internal failure.
pub async fn login_finish(
    State(state): State<AuthState>,
    Json(req): Json<LoginFinishRequest>,
) -> Result<Json<LoginFinishResponse>, ApiError> {
    // An unknown/expired login_id is a uniform auth failure — no identity
    // context to attribute a throttle failure to, so just reject.
    let Some((server_login, identity_handle)) = state.take_pending(&req.login_id)? else {
        return Err(ApiError::unauthorized("authentication failed"));
    };

    // Re-check the throttle in case a lockout began between start and finish.
    let locked = state
        .lock_store()?
        .locked_until(&identity_handle)?
        .is_some();
    if locked {
        return Err(ApiError::too_many_requests(
            "too many attempts; try again later",
        ));
    }

    let finalization_bytes = decode_b64(
        "credential_finalization_b64",
        &req.credential_finalization_b64,
    )?;
    let finalization =
        CredentialFinalization::<PergamonCipherSuite>::deserialize(&finalization_bytes)
            .map_err(|_| ApiError::bad_request("malformed credential finalization"))?;

    // WP-3b/#192 will bind the mutually-authenticated `session_key` to a
    // device-scoped bearer token here. For WP-3a we only need to know the AKE
    // completed (identity proven); the failure path is deliberately uniform.
    let Ok(_finished) = server_login.finish(finalization, ServerLoginParameters::default()) else {
        // Wrong password or dummy-path (unknown identity): identical outcome.
        let mut store = state.lock_store()?;
        store.record_failure(&identity_handle, state.throttle())?;
        drop(store);
        return Err(ApiError::unauthorized("authentication failed"));
    };

    // The mutually-authenticated session key lives in `_finished.session_key`.
    let mut store = state.lock_store()?;
    store.reset_failures(&identity_handle)?;
    let account_id = store
        .account_id(&identity_handle)?
        .ok_or_else(|| ApiError::internal("authenticated identity has no account mapping"))?;
    drop(store);
    Ok(Json(LoginFinishResponse {
        authenticated: true,
        account_id,
    }))
}
