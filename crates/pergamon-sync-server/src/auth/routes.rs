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
//! A successful login mints a per-device bearer + refresh token bundle bound to
//! the ADR-024 Ed25519 device key, **iff** the request carries a valid
//! proof-of-possession (`PoP`). Without a `PoP` the login behaves exactly as in
//! WP-3a (authenticated, no token). [`token_refresh`] and [`token_revoke`]
//! complete the lifecycle. See [`crate::auth::token`] for the `PoP` binding and
//! [`crate::auth::store`] for persistence.
//!
//! ## Enforcement seam — WP-3c/#197
//! This WP mints/refreshes/revokes tokens and exposes
//! [`crate::auth::store::AuthStore::validate_token`] as the primitive, but does
//! **not** gate the blind content routes on it. Putting every `{account_id}`
//! content route behind that primitive (asserting `token.account_id == route
//! account_id`) is WP-3c ([#197]).
//!
//! [#192]: https://github.com/kafkade/pergamon/issues/192
//! [#197]: https://github.com/kafkade/pergamon/issues/197

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use opaque_ke::{
    CredentialFinalization, CredentialRequest, RegistrationRequest, RegistrationUpload,
    ServerLogin, ServerLoginParameters, ServerRegistration,
};
use rand::rngs::OsRng;

use crate::auth::PergamonCipherSuite;
use crate::auth::state::AuthState;
use crate::auth::store::AuthStore;
use crate::auth::token::{self, TokenKind};
use crate::auth::wire::{
    LoginFinishRequest, LoginFinishResponse, LoginStartRequest, LoginStartResponse, RefreshRequest,
    RefreshResponse, RegisterFinishRequest, RegisterFinishResponse, RegisterStartRequest,
    RegisterStartResponse, RevokeRequest, RevokeResponse, TokenBundle,
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

/// Decode a base64 field into a fixed-size byte array, mapping failures to 400.
fn decode_fixed<const N: usize>(field: &str, value: &str) -> Result<[u8; N], ApiError> {
    let bytes = decode_b64(field, value)?;
    <[u8; N]>::try_from(bytes)
        .map_err(|_| ApiError::bad_request(format!("{field} must be {N} bytes")))
}

/// Extract the bearer token from an `Authorization: Bearer <token>` header.
fn bearer_from_headers(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing authorization header"))?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .ok_or_else(|| ApiError::unauthorized("expected a Bearer authorization header"))?;
    Ok(token.trim().to_string())
}

/// Mint a per-device token bundle on a successful login **iff** the request
/// carries a complete, valid device proof-of-possession (WP-3b, #192).
///
/// Returns `Ok(None)` when no `PoP` is present (the WP-3a behavior: authenticate,
/// mint nothing). Returns an error (uniform `401`) when a `PoP` *is* present but
/// invalid — a partial/forged `PoP` never yields a token. Because this runs only
/// after the AKE succeeded, it is unreachable by an unauthenticated caller, so a
/// distinct PoP-failure outcome here is not an account-existence oracle.
fn mint_on_login(
    state: &AuthState,
    store: &mut AuthStore,
    req: &LoginFinishRequest,
    credential_finalization: &[u8],
    account_id: &str,
) -> Result<Option<TokenBundle>, ApiError> {
    // A token is minted only when ALL three PoP fields are present. Absent PoP =
    // WP-3a behavior (no token). A *partial* PoP is a malformed request.
    let (device_id, pub_b64, sig_b64) = match (
        req.device_id.as_deref(),
        req.ed25519_pub_b64.as_deref(),
        req.pop_signature_b64.as_deref(),
    ) {
        (None, None, None) => return Ok(None),
        (Some(d), Some(p), Some(s)) => (d, p, s),
        _ => {
            return Err(ApiError::bad_request(
                "device proof-of-possession requires device_id, ed25519_pub_b64, and \
                 pop_signature_b64 together",
            ));
        }
    };

    let ed25519_pub = decode_fixed::<{ token::ED25519_PUB_LEN }>("ed25519_pub_b64", pub_b64)?;
    let signature = decode_fixed::<{ token::ED25519_SIG_LEN }>("pop_signature_b64", sig_b64)?;

    // The device_id must be cryptographically bound to the presented key, and
    // the signature must verify over the single-use mint-PoP message.
    let pop_ok = token::device_id_from_ed25519(&ed25519_pub) == device_id
        && token::verify_ed25519(
            &ed25519_pub,
            &token::mint_pop_message(
                &req.login_id,
                credential_finalization,
                device_id,
                &ed25519_pub,
            ),
            &signature,
        );
    if !pop_ok {
        return Err(ApiError::unauthorized("device proof-of-possession failed"));
    }

    let cfg = *state.token_config();
    let (access_token, access_expires_at) = store.mint(
        account_id,
        device_id,
        &ed25519_pub,
        TokenKind::Access,
        cfg.access_ttl_ms,
    )?;
    let (refresh_token, refresh_expires_at) = store.mint(
        account_id,
        device_id,
        &ed25519_pub,
        TokenKind::Refresh,
        cfg.refresh_ttl_ms,
    )?;

    Ok(Some(TokenBundle {
        access_token,
        access_expires_at,
        refresh_token,
        refresh_expires_at,
        device_id: device_id.to_string(),
        account_id: account_id.to_string(),
    }))
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

    // WP-3b/#192: bind the mutually-authenticated login to a device-scoped
    // token bundle here. For WP-3a we only need to know the AKE completed
    // (identity proven); the failure path is deliberately uniform.
    let Ok(_finished) = server_login.finish(finalization, ServerLoginParameters::default()) else {
        // Wrong password or dummy-path (unknown identity): identical outcome.
        let mut store = state.lock_store()?;
        store.record_failure(&identity_handle, state.throttle())?;
        drop(store);
        return Err(ApiError::unauthorized("authentication failed"));
    };

    // The mutually-authenticated session key lives in `_finished.session_key`;
    // WP-3b does not need it (the device PoP, not the session key, binds the
    // token to a device) and deliberately never derives content keys from it.
    let mut store = state.lock_store()?;
    store.reset_failures(&identity_handle)?;
    let account_id = store
        .account_id(&identity_handle)?
        .ok_or_else(|| ApiError::internal("authenticated identity has no account mapping"))?;
    // Mint a per-device token bundle iff a valid device PoP accompanies the login.
    let token = mint_on_login(&state, &mut store, &req, &finalization_bytes, &account_id)?;
    drop(store);
    Ok(Json(LoginFinishResponse {
        authenticated: true,
        account_id,
        token,
    }))
}

/// `POST /v1/auth/token/refresh` — exchange a valid refresh token plus a fresh
/// device proof-of-possession for a new short-lived access token, **rotating**
/// the refresh token in the process (WP-3b, #192).
///
/// The refresh token must exist, be of kind `refresh`, be unexpired, and not be
/// revoked; the presented device key must match the one the refresh token is
/// bound to; and the refresh-PoP signature must verify. On success the presented
/// refresh token is revoked and a fresh access + refresh pair is returned in one
/// transaction, so each refresh secret is **single-use**: a captured refresh
/// token is bounded to a single exchange, and replaying an already-rotated
/// refresh token fails here. A revoked device's refresh token is likewise
/// rejected, so revocation immediately stops new access tokens.
///
/// ## Hardening seam (not yet implemented)
/// Refresh-token *reuse detection* (if an already-rotated/revoked refresh token
/// is presented again with a valid signature, treat it as a theft signal and
/// revoke the whole device family) and server-side refresh-nonce tracking are
/// further hardening steps left for external review / a follow-up. Today the
/// refresh secret plus single-use rotation is the gate.
///
/// # Errors
/// Returns 400 on malformed input; 401 on an invalid/expired/revoked refresh
/// token, a device mismatch, or a failed `PoP`; 500 on an internal failure.
pub async fn token_refresh(
    State(state): State<AuthState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>, ApiError> {
    let ed25519_pub =
        decode_fixed::<{ token::ED25519_PUB_LEN }>("ed25519_pub_b64", &req.ed25519_pub_b64)?;
    let signature =
        decode_fixed::<{ token::ED25519_SIG_LEN }>("pop_signature_b64", &req.pop_signature_b64)?;
    let nonce = decode_b64("nonce_b64", &req.nonce_b64)?;

    // The presented device_id must be bound to the presented key.
    if token::device_id_from_ed25519(&ed25519_pub) != req.device_id {
        return Err(ApiError::unauthorized("device proof-of-possession failed"));
    }

    let mut store = state.lock_store()?;
    let Some(refresh) = store.load_valid_token(&req.refresh_token, TokenKind::Refresh)? else {
        return Err(ApiError::unauthorized("invalid refresh token"));
    };
    // The refresh token must be bound to exactly this device + key.
    if refresh.device_id != req.device_id || refresh.ed25519_pub != ed25519_pub {
        return Err(ApiError::unauthorized("refresh token device mismatch"));
    }
    // Verify the fresh refresh-PoP over the (single) refresh token id + nonce.
    let msg = token::refresh_pop_message(&refresh.token_id, &nonce, &req.device_id, &ed25519_pub);
    if !token::verify_ed25519(&ed25519_pub, &msg, &signature) {
        return Err(ApiError::unauthorized("device proof-of-possession failed"));
    }

    // Rotate: revoke the presented refresh token and issue a fresh access +
    // refresh pair atomically (single-use refresh secrets).
    let cfg = *state.token_config();
    let rotated = store.rotate_refresh(
        &refresh.token_id,
        &refresh.account_id,
        &refresh.device_id,
        &ed25519_pub,
        cfg.access_ttl_ms,
        cfg.refresh_ttl_ms,
    )?;
    drop(store);
    Ok(Json(RefreshResponse {
        access_token: rotated.access_token,
        access_expires_at: rotated.access_expires_at,
        refresh_token: rotated.refresh_token,
        refresh_expires_at: rotated.refresh_expires_at,
    }))
}

/// `POST /v1/auth/token/revoke` — revoke all tokens for a device within the
/// caller's own account (WP-3b, #192).
///
/// Authenticated by a valid access-token bearer in the `Authorization` header.
/// The target `device_id` is revoked **only** within the caller's account
/// (resolved from the bearer), so one tenant can never revoke another's tokens.
/// After revocation the device's access token fails `validate_token` and its
/// refresh token can no longer mint access tokens.
///
/// # Errors
/// Returns 401 when the bearer is missing/invalid; 500 on an internal failure.
pub async fn token_revoke(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(req): Json<RevokeRequest>,
) -> Result<Json<RevokeResponse>, ApiError> {
    let bearer = bearer_from_headers(&headers)?;
    let caller = state
        .validate_token(&bearer)?
        .ok_or_else(|| ApiError::unauthorized("invalid or missing access token"))?;

    let mut store = state.lock_store()?;
    let revoked = store.revoke_device(&caller.account_id, &req.device_id)?;
    drop(store);
    Ok(Json(RevokeResponse {
        revoked: u64::try_from(revoked).unwrap_or(u64::MAX),
    }))
}
