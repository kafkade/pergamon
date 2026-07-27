// SPDX-License-Identifier: AGPL-3.0-only

//! Wire types for the OPAQUE auth endpoints (WP-3a, #189).
//!
//! Every OPAQUE protocol message is carried as an opaque, standard-base64 string
//! (`*_b64`). The server (de)serializes these through `opaque-ke`; it never
//! learns the password. Responses are deliberately uniform between existing and
//! unknown identities (design §1.6) — the field shapes below do not vary with
//! account existence.

use serde::{Deserialize, Serialize};

/// `POST /v1/auth/register/start` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterStartRequest {
    /// The login identity handle (an opaque username; email not required).
    pub identity_handle: String,
    /// Base64 `RegistrationRequest` from `ClientRegistration::start`.
    pub registration_request_b64: String,
}

/// `POST /v1/auth/register/start` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterStartResponse {
    /// Base64 `RegistrationResponse` from `ServerRegistration::start`.
    pub registration_response_b64: String,
}

/// `POST /v1/auth/register/finish` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterFinishRequest {
    /// The login identity handle from the start step (the database key).
    pub identity_handle: String,
    /// Base64 `RegistrationUpload` from `ClientRegistration::finish`.
    pub registration_upload_b64: String,
}

/// `POST /v1/auth/register/finish` response.
///
/// Returns the freshly allocated opaque `account_id` to its owner (the party
/// that just completed registration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterFinishResponse {
    /// The allocated opaque content-plane account handle.
    pub account_id: String,
}

/// `POST /v1/auth/login/start` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginStartRequest {
    /// The login identity handle being authenticated.
    pub identity_handle: String,
    /// Base64 `CredentialRequest` (KE1) from `ClientLogin::start`.
    pub credential_request_b64: String,
}

/// `POST /v1/auth/login/start` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginStartResponse {
    /// Opaque handle correlating this login's start and finish steps.
    pub login_id: String,
    /// Base64 `CredentialResponse` (KE2) from `ServerLogin::start`. For an
    /// unknown identity this is a dummy response, indistinguishable from a real
    /// one (design §1.6).
    pub credential_response_b64: String,
}

/// `POST /v1/auth/login/finish` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginFinishRequest {
    /// The `login_id` returned by the start step.
    pub login_id: String,
    /// Base64 `CredentialFinalization` (KE3) from `ClientLogin::finish`.
    pub credential_finalization_b64: String,
}

/// `POST /v1/auth/login/finish` response (only returned on success).
///
/// WP-3a establishes an authenticated session; minting the actual per-device
/// bearer token bound to the ADR-024 Ed25519 key is WP-3b/#192. Until then a
/// successful login returns the authenticated account's opaque `account_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginFinishResponse {
    /// Always `true` on success (the failure path returns an error status).
    pub authenticated: bool,
    /// The authenticated account's opaque content-plane handle.
    pub account_id: String,
}
