// SPDX-License-Identifier: Apache-2.0

//! OPAQUE client helpers for hosted-sync server auth (WP-3a, #189).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! This is the **client** half of the OPAQUE flows whose server half lives in
//! the AGPL `pergamon-sync-server` crate. It is gated behind the `auth` feature
//! so the core sync engine stays crypto-light. `opaque-ke` is dual
//! MIT/Apache-2.0, so it is fine in this Apache-2.0 crate; nothing here crosses
//! the AGPL/Apache boundary.
//!
//! The password (optionally folded with a high-entropy Secret Key by a higher
//! layer — the server is agnostic, design §Part 4 Q2) never leaves the device:
//! the OPRF blinds it before it is sent.
//!
//! ## Cross-crate cipher-suite parity
//! [`PergamonCipherSuite`] must stay byte-for-byte parameter-identical to the
//! server's definition in `pergamon-sync-server::auth::cipher_suite`. The two
//! are deliberately duplicated to respect the AGPL/Apache split; the server↔
//! client round-trip integration test guards against drift. **Keep them in
//! sync.**
//!
//! Because `opaque-ke 4.x` is built on the `digest 0.10` generation, the AKE
//! hash is `sha2 0.10`'s `Sha512`, imported here via the `sha2-opaque` rename.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use opaque_ke::{
    CipherSuite, ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse, Ristretto255,
    TripleDh,
};
use pergamon_crypto::device::DeviceKeypairs;
use pergamon_crypto::primitives::blake3_hash;
use rand::RngCore as _;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

/// The project OPAQUE cipher suite — identical to the server's (see module docs).
#[derive(Debug, Clone, Copy)]
pub struct PergamonCipherSuite;

impl CipherSuite for PergamonCipherSuite {
    type OprfCs = Ristretto255;
    type KeyExchange = TripleDh<Ristretto255, sha2_opaque::Sha512>;
    type Ksf = argon2::Argon2<'static>;
}

/// Errors from the client OPAQUE helpers.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The `opaque-ke` protocol rejected a message (e.g. an invalid login).
    #[error("OPAQUE protocol error: {0}")]
    Protocol(String),
    /// A server message could not be decoded.
    #[error("failed to decode OPAQUE server message")]
    Decode,
}

/// An in-progress client registration awaiting the server's response.
///
/// Hold this between [`ClientRegistrationFlow::start`] and
/// [`ClientRegistrationFlow::finish`].
pub struct ClientRegistrationFlow {
    state: ClientRegistration<PergamonCipherSuite>,
}

impl ClientRegistrationFlow {
    /// Begin registration for `password`. Returns the flow to persist and the
    /// serialized `RegistrationRequest` to send to `register/start`.
    ///
    /// # Errors
    /// Returns [`AuthError::Protocol`] if the OPRF blinding fails.
    pub fn start(password: &[u8]) -> Result<(Self, Vec<u8>), AuthError> {
        let mut rng = OsRng;
        let result = ClientRegistration::<PergamonCipherSuite>::start(&mut rng, password)
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok((
            Self {
                state: result.state,
            },
            result.message.serialize().to_vec(),
        ))
    }

    /// Finish registration given the server's `RegistrationResponse` bytes.
    /// Returns the serialized `RegistrationUpload` to send to `register/finish`.
    ///
    /// # Errors
    /// Returns [`AuthError::Decode`] if the response is malformed, or
    /// [`AuthError::Protocol`] if finalization fails.
    pub fn finish(self, password: &[u8], response_bytes: &[u8]) -> Result<Vec<u8>, AuthError> {
        let response = RegistrationResponse::<PergamonCipherSuite>::deserialize(response_bytes)
            .map_err(|_| AuthError::Decode)?;
        let mut rng = OsRng;
        let result = self
            .state
            .finish(
                &mut rng,
                password,
                response,
                ClientRegistrationFinishParameters::default(),
            )
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok(result.message.serialize().to_vec())
    }
}

/// An in-progress client login awaiting the server's KE2 response.
///
/// Hold this between [`ClientLoginFlow::start`] and [`ClientLoginFlow::finish`].
pub struct ClientLoginFlow {
    state: ClientLogin<PergamonCipherSuite>,
}

/// The successful result of [`ClientLoginFlow::finish`].
pub struct ClientLoginFinished {
    /// Serialized `CredentialFinalization` (KE3) to send to `login/finish`.
    pub finalization: Vec<u8>,
    /// The mutually-authenticated session key (matches the server's on success).
    pub session_key: Vec<u8>,
}

impl ClientLoginFlow {
    /// Begin login for `password`. Returns the flow to persist and the
    /// serialized `CredentialRequest` (KE1) to send to `login/start`.
    ///
    /// # Errors
    /// Returns [`AuthError::Protocol`] if the OPRF blinding fails.
    pub fn start(password: &[u8]) -> Result<(Self, Vec<u8>), AuthError> {
        let mut rng = OsRng;
        let result = ClientLogin::<PergamonCipherSuite>::start(&mut rng, password)
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok((
            Self {
                state: result.state,
            },
            result.message.serialize().to_vec(),
        ))
    }

    /// Finish login given the server's `CredentialResponse` (KE2) bytes.
    ///
    /// # Errors
    /// Returns [`AuthError::Decode`] if the response is malformed, or
    /// [`AuthError::Protocol`] on an invalid login (wrong password / unknown
    /// identity dummy path) — the two are indistinguishable by design.
    pub fn finish(
        self,
        password: &[u8],
        response_bytes: &[u8],
    ) -> Result<ClientLoginFinished, AuthError> {
        let response = CredentialResponse::<PergamonCipherSuite>::deserialize(response_bytes)
            .map_err(|_| AuthError::Decode)?;
        let mut rng = OsRng;
        let result = self
            .state
            .finish(
                &mut rng,
                password,
                response,
                ClientLoginFinishParameters::default(),
            )
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok(ClientLoginFinished {
            finalization: result.message.serialize().to_vec(),
            session_key: result.session_key.to_vec(),
        })
    }
}

// ===========================================================================
// WP-3b (#192): per-device token proof-of-possession (client half)
// ===========================================================================
//
// # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//
// The device signs a domain-tagged, single-use message with its ADR-024 Ed25519
// key to prove possession at token **issuance** (folded into `login/finish`) and
// at token **refresh**. The binding bytes below are **byte-for-byte identical**
// to the server's `pergamon-sync-server::auth::token` (deliberately duplicated
// to respect the AGPL/Apache boundary); the server↔client interop test
// (`e2e_tokens.rs`) is the guardrail against drift. **Keep them in sync.**
//
// This is the Apache client half: it produces the PoP signatures and the request
// bodies, and parses the token responses. It never touches the ARK or any
// content key — a session token authorizes the account for the server (quotas /
// billing) but the auth plane stays orthogonal to the content plane (ADR-029).

/// Domain tag for the token-issuance (mint) `PoP` message. Must equal the server's.
const MINT_POP_TAG: &[u8] = b"pergamon/v1/auth/token-mint-pop";

/// Domain tag for the token-refresh `PoP` message. Must equal the server's.
const REFRESH_POP_TAG: &[u8] = b"pergamon/v1/auth/token-refresh-pop";

/// Length in bytes of an Ed25519 public key.
pub const ED25519_PUB_LEN: usize = 32;

/// Append a big-endian `u32` length prefix followed by the bytes.
fn push_len_prefixed(buf: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// Build the domain-tagged bytes a device signs to prove possession at token
/// **issuance** (mint). Byte-identical to the server's `mint_pop_message`.
///
/// `tag || len(login_id)||login_id || blake3(KE3) || len(device_id)||device_id ||
/// ed25519_pub`.
#[must_use]
pub fn mint_pop_message(
    login_id: &str,
    credential_finalization: &[u8],
    device_id: &str,
    ed25519_pub: &[u8; ED25519_PUB_LEN],
) -> Vec<u8> {
    let ke3_hash = blake3_hash(credential_finalization);
    let mut msg = Vec::with_capacity(MINT_POP_TAG.len() + 64 + login_id.len() + device_id.len());
    msg.extend_from_slice(MINT_POP_TAG);
    push_len_prefixed(&mut msg, login_id.as_bytes());
    msg.extend_from_slice(&ke3_hash);
    push_len_prefixed(&mut msg, device_id.as_bytes());
    msg.extend_from_slice(ed25519_pub);
    msg
}

/// Build the domain-tagged bytes a device signs to prove possession at token
/// **refresh**. Byte-identical to the server's `refresh_pop_message`.
///
/// `tag || len(refresh_token_id)||refresh_token_id || len(nonce)||nonce ||
/// len(device_id)||device_id || ed25519_pub`.
#[must_use]
pub fn refresh_pop_message(
    refresh_token_id: &str,
    nonce: &[u8],
    device_id: &str,
    ed25519_pub: &[u8; ED25519_PUB_LEN],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(
        REFRESH_POP_TAG.len() + refresh_token_id.len() + nonce.len() + device_id.len() + 40,
    );
    msg.extend_from_slice(REFRESH_POP_TAG);
    push_len_prefixed(&mut msg, refresh_token_id.as_bytes());
    push_len_prefixed(&mut msg, nonce);
    push_len_prefixed(&mut msg, device_id.as_bytes());
    msg.extend_from_slice(ed25519_pub);
    msg
}

/// The stable, non-secret id portion of an opaque bearer string
/// `"{token_id}.{secret_b64url}"`. This is what the refresh-PoP is bound to.
#[must_use]
pub fn token_id_from_bearer(bearer: &str) -> Option<&str> {
    let (id, _) = bearer.split_once('.')?;
    (!id.is_empty()).then_some(id)
}

/// A fresh 16-byte nonce for a refresh-PoP (OS CSPRNG).
#[must_use]
pub fn fresh_nonce() -> [u8; 16] {
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// The device proof-of-possession fields to attach to a `login/finish` request
/// so the server mints a per-device token bundle (WP-3b, #192).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceMintPop {
    /// The ADR-024 `device_id` (equals `blake3(ed25519_pub)[..16]`).
    pub device_id: String,
    /// Base64 of the device's 32-byte Ed25519 public key.
    pub ed25519_pub_b64: String,
    /// Base64 of the device's Ed25519 signature over the mint-PoP message.
    pub pop_signature_b64: String,
}

/// Produce the [`DeviceMintPop`] for a `login/finish`, signing the single-use
/// mint-PoP (bound to `login_id` and the exact KE3 transcript) with `device`.
#[must_use]
pub fn build_mint_pop(
    device: &DeviceKeypairs,
    login_id: &str,
    credential_finalization: &[u8],
) -> DeviceMintPop {
    let ed25519_pub = device.ed25519_verifying();
    let msg = mint_pop_message(
        login_id,
        credential_finalization,
        device.device_id(),
        ed25519_pub,
    );
    let signature = device.sign(&msg);
    DeviceMintPop {
        device_id: device.device_id().to_string(),
        ed25519_pub_b64: STANDARD.encode(ed25519_pub),
        pop_signature_b64: STANDARD.encode(signature),
    }
}

/// A `POST /v1/auth/token/refresh` request body (mirror of the server's
/// `RefreshRequest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequestBody {
    /// The opaque refresh-token bearer string previously issued.
    pub refresh_token: String,
    /// The ADR-024 `device_id` (must match the refresh token's bound device).
    pub device_id: String,
    /// Base64 of the device's 32-byte Ed25519 public key.
    pub ed25519_pub_b64: String,
    /// Base64 of the fresh nonce mixed into the refresh-PoP message.
    pub nonce_b64: String,
    /// Base64 of the device's Ed25519 signature over the refresh-PoP message.
    pub pop_signature_b64: String,
}

/// Build a signed `token/refresh` request for `refresh_token` using a fresh
/// nonce and `device`'s Ed25519 key.
#[must_use]
pub fn build_refresh_request(
    device: &DeviceKeypairs,
    refresh_token: &str,
    nonce: &[u8],
) -> RefreshRequestBody {
    let ed25519_pub = device.ed25519_verifying();
    let refresh_token_id = token_id_from_bearer(refresh_token).unwrap_or_default();
    let msg = refresh_pop_message(refresh_token_id, nonce, device.device_id(), ed25519_pub);
    let signature = device.sign(&msg);
    RefreshRequestBody {
        refresh_token: refresh_token.to_string(),
        device_id: device.device_id().to_string(),
        ed25519_pub_b64: STANDARD.encode(ed25519_pub),
        nonce_b64: STANDARD.encode(nonce),
        pop_signature_b64: STANDARD.encode(signature),
    }
}

/// A minted per-device token bundle returned by `login/finish` (mirror of the
/// server's `TokenBundle`).
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

/// A `token/refresh` response (mirror of the server's `RefreshResponse`).
///
/// The refresh token is rotated on every use: `refresh_token` **replaces** the
/// one just presented (which the server revokes). Clients must persist it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResponse {
    /// A new opaque access-token bearer string.
    pub access_token: String,
    /// The new access token's expiry, epoch milliseconds.
    pub access_expires_at: i64,
    /// A new opaque refresh-token bearer string that replaces the presented one.
    pub refresh_token: String,
    /// The new refresh token's expiry, epoch milliseconds.
    pub refresh_expires_at: i64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A local client-only sanity check that the helpers compose (the full
    /// cross-crate round trip against the server is proven in the server crate's
    /// integration test). Here we only drive the client side against the
    /// library's own server types to keep this crate self-contained.
    #[test]
    fn client_helpers_produce_messages() {
        use opaque_ke::{ServerRegistration, ServerSetup};

        let (reg_flow, request) = ClientRegistrationFlow::start(b"pw").unwrap();
        assert!(!request.is_empty());

        // Drive the server side with the library directly to get a response.
        let mut rng = OsRng;
        let setup = ServerSetup::<PergamonCipherSuite>::new(&mut rng);
        let req =
            opaque_ke::RegistrationRequest::<PergamonCipherSuite>::deserialize(&request).unwrap();
        let s_start =
            ServerRegistration::<PergamonCipherSuite>::start(&setup, req, b"alice").unwrap();
        let upload = reg_flow
            .finish(b"pw", &s_start.message.serialize())
            .unwrap();
        assert!(!upload.is_empty());
    }

    /// WP-3b: the mint-PoP binding is stable and the device signature verifies
    /// against the presented Ed25519 key (the full server round trip is proven
    /// by the server crate's `e2e_tokens.rs`).
    #[test]
    fn mint_pop_signs_and_verifies() {
        let device = DeviceKeypairs::generate().unwrap();
        let pop = build_mint_pop(&device, "login-abc", b"ke3-transcript-bytes");
        assert_eq!(pop.device_id, device.device_id());

        let pubkey = device.ed25519_verifying();
        let sig_bytes = STANDARD.decode(pop.pop_signature_b64.as_bytes()).unwrap();
        let sig: [u8; 64] = sig_bytes.try_into().unwrap();
        let msg = mint_pop_message(
            "login-abc",
            b"ke3-transcript-bytes",
            device.device_id(),
            pubkey,
        );
        assert!(pergamon_crypto::primitives::ed25519_verify(pubkey, &msg, &sig).is_ok());

        // Binding to the login_id makes the signature non-replayable: a different
        // login_id yields a different message that the same signature fails.
        let other = mint_pop_message(
            "login-XYZ",
            b"ke3-transcript-bytes",
            device.device_id(),
            pubkey,
        );
        assert!(pergamon_crypto::primitives::ed25519_verify(pubkey, &other, &sig).is_err());
    }

    /// WP-3b: the refresh-PoP binding is stable, verifies, and the refresh-token
    /// id is parsed from the bearer string the same way both sides expect.
    #[test]
    fn refresh_pop_signs_and_verifies() {
        let device = DeviceKeypairs::generate().unwrap();
        let refresh_token = "abc123.c2VjcmV0"; // {token_id}.{secret_b64url}
        assert_eq!(token_id_from_bearer(refresh_token), Some("abc123"));

        let nonce = fresh_nonce();
        let body = build_refresh_request(&device, refresh_token, &nonce);
        assert_eq!(body.device_id, device.device_id());

        let pubkey = device.ed25519_verifying();
        let sig_bytes = STANDARD.decode(body.pop_signature_b64.as_bytes()).unwrap();
        let sig: [u8; 64] = sig_bytes.try_into().unwrap();
        let msg = refresh_pop_message("abc123", &nonce, device.device_id(), pubkey);
        assert!(pergamon_crypto::primitives::ed25519_verify(pubkey, &msg, &sig).is_ok());
    }
}
