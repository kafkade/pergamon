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

    // --- Device proof-of-possession for token issuance (WP-3b, #192) ---------
    // These are OPTIONAL at the wire level but REQUIRED for token issuance: a
    // login without them authenticates exactly as in WP-3a and mints no token
    // (the WP-3a behavior and its tests are unchanged). When all three are
    // present the server verifies the PoP (design §2.2) and, only then, mints a
    // per-device token bundle. A token can therefore NEVER be minted without a
    // valid device-key proof-of-possession.
    /// The ADR-024 `device_id` requesting a token (must equal
    /// `blake3(ed25519_pub)[..16]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Base64 of the device's 32-byte Ed25519 public key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ed25519_pub_b64: Option<String>,
    /// Base64 of the device's 64-byte Ed25519 signature over the mint-PoP message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pop_signature_b64: Option<String>,
}

/// A minted per-device token bundle (WP-3b, #192): a short-lived access token
/// plus a longer-lived refresh token, both scoped to one `account_id` and bound
/// to the requesting device's Ed25519 key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBundle {
    /// Opaque access-token bearer string.
    pub access_token: String,
    /// Access-token expiry, epoch milliseconds.
    pub access_expires_at: i64,
    /// Opaque refresh-token bearer string.
    pub refresh_token: String,
    /// Refresh-token expiry, epoch milliseconds.
    pub refresh_expires_at: i64,
    /// The device the tokens are bound to.
    pub device_id: String,
    /// The single account the tokens authorize.
    pub account_id: String,
}

/// `POST /v1/auth/login/finish` response (only returned on success).
///
/// WP-3a establishes an authenticated session and returns the opaque
/// `account_id`. WP-3b additionally returns a [`TokenBundle`] **iff** the request
/// carried a valid device proof-of-possession; otherwise `token` is omitted and
/// the response is byte-identical to WP-3a's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginFinishResponse {
    /// Always `true` on success (the failure path returns an error status).
    pub authenticated: bool,
    /// The authenticated account's opaque content-plane handle.
    pub account_id: String,
    /// The minted per-device token bundle, if a valid `PoP` was presented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenBundle>,
}

/// `POST /v1/auth/token/refresh` request (WP-3b, #192).
///
/// Exchanges a valid, unrevoked refresh token — plus a **fresh** device
/// proof-of-possession — for a new short-lived access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequest {
    /// The opaque refresh-token bearer string previously issued.
    pub refresh_token: String,
    /// The ADR-024 `device_id` (must match the refresh token's bound device).
    pub device_id: String,
    /// Base64 of the device's 32-byte Ed25519 public key (must match the bound
    /// key).
    pub ed25519_pub_b64: String,
    /// Base64 of a client-chosen fresh nonce mixed into the refresh-PoP message.
    pub nonce_b64: String,
    /// Base64 of the device's Ed25519 signature over the refresh-PoP message.
    pub pop_signature_b64: String,
}

/// `POST /v1/auth/token/refresh` response.
///
/// The refresh token is **rotated** on every successful use (WP-3b hardening):
/// the presented refresh token is revoked and a fresh one is returned alongside
/// the new access token, so each refresh secret is single-use. Clients MUST
/// replace their stored refresh token with `refresh_token` here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResponse {
    /// A new opaque access-token bearer string.
    pub access_token: String,
    /// The new access token's expiry, epoch milliseconds.
    pub access_expires_at: i64,
    /// A new opaque refresh-token bearer string that **replaces** the presented
    /// one (which is now revoked).
    pub refresh_token: String,
    /// The new refresh token's expiry, epoch milliseconds.
    pub refresh_expires_at: i64,
}

/// `POST /v1/auth/token/revoke` request (WP-3b, #192).
///
/// Authenticated by a valid access-token bearer in the `Authorization` header;
/// revokes all tokens for `device_id` **within the caller's own account**.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeRequest {
    /// The device whose tokens should be revoked (self-revoke is the subset).
    pub device_id: String,
}

/// `POST /v1/auth/token/revoke` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeResponse {
    /// Number of tokens revoked by this call.
    pub revoked: u64,
}
