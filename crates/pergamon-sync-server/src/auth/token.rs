// SPDX-License-Identifier: AGPL-3.0-only

//! Per-device bearer/refresh token primitives (WP-3b, #192).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! This module implements the pure/crypto half of the per-device credential
//! model from `docs/design/hosted-auth-control-plane.md` §2.2–§2.3: the token
//! wire format, the token-secret hashing, the server-side `device_id`
//! derivation, the Ed25519 proof-of-possession (`PoP`) binding, and signature
//! verification. The stateful half (persistence, revocation) lives in
//! [`crate::auth::store`]; the HTTP wiring lives in [`crate::auth::routes`].
//!
//! ## Auth plane ⟂ content plane (ADR-024 / ADR-029)
//! A token binds a login to a **device signing key** and one opaque
//! `account_id`. It **never** touches the ARK or any content key: the server can
//! prove a caller controls the account (for quotas/billing) yet remains unable
//! to derive content keys. Nothing in this module reads or derives content keys.
//!
//! ## Device-key binding (proof-of-possession)
//! Issuance and refresh both require a signature over a **domain-tagged,
//! single-use** message using the device's ADR-024 Ed25519 key — the same key
//! that signs the device record already relayed to the server. A stolen bearer
//! token alone (without the device private key) cannot be refreshed, and the
//! `device_id` a token binds to is cryptographically pinned to that key
//! (`device_id == blake3(ed25519_pub)[..16]`), replicated here server-side so the
//! AGPL server never links the Apache `pergamon-crypto` crate at runtime.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, VerifyingKey};
use rand::RngCore as _;
use rand::rngs::OsRng;
use uuid::Uuid;

/// Length in bytes of a token secret (256-bit, high-entropy).
pub const TOKEN_SECRET_LEN: usize = 32;

/// Length in bytes of the raw `device_id` handle before hex encoding (128-bit).
/// Mirrors `pergamon_crypto::device::DEVICE_ID_LEN`.
pub const DEVICE_ID_LEN: usize = 16;

/// Length in bytes of an Ed25519 public key.
pub const ED25519_PUB_LEN: usize = 32;

/// Length in bytes of an Ed25519 signature.
pub const ED25519_SIG_LEN: usize = 64;

/// Domain tag for the token-issuance (mint) `PoP` message.
const MINT_POP_TAG: &[u8] = b"pergamon/v1/auth/token-mint-pop";

/// Domain tag for the token-refresh `PoP` message.
const REFRESH_POP_TAG: &[u8] = b"pergamon/v1/auth/token-refresh-pop";

/// Lifetime policy for minted tokens (design §2.2: short-lived access tokens
/// plus a longer-lived refresh path).
#[derive(Debug, Clone, Copy)]
pub struct TokenConfig {
    /// Access-token time-to-live, in milliseconds.
    pub access_ttl_ms: i64,
    /// Refresh-token time-to-live, in milliseconds.
    pub refresh_ttl_ms: i64,
}

impl Default for TokenConfig {
    fn default() -> Self {
        Self {
            // Short-lived access token: 1 hour.
            access_ttl_ms: 60 * 60 * 1000,
            // Longer-lived refresh token: 30 days.
            refresh_ttl_ms: 30 * 24 * 60 * 60 * 1000,
        }
    }
}

/// Which kind of token a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Short-lived credential presented on content requests.
    Access,
    /// Longer-lived credential used only to mint fresh access tokens.
    Refresh,
}

impl TokenKind {
    /// Stable string form persisted in the store.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Refresh => "refresh",
        }
    }

    /// Parse the persisted string form.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "access" => Some(Self::Access),
            "refresh" => Some(Self::Refresh),
            _ => None,
        }
    }
}

/// The authenticated principal a validated bearer token resolves to.
///
/// This is the reusable primitive WP-3c ([#197]) consumes to gate the blind
/// content routes: it asserts `account_id` equals the `{account_id}` a route
/// targets before any handler touches that tenant's data.
///
/// [#197]: https://github.com/kafkade/pergamon/issues/197
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthAccount {
    /// The single opaque content-plane account this token authorizes.
    pub account_id: String,
    /// The ADR-024 device the token is bound to.
    pub device_id: String,
}

/// A freshly generated token: the parts to persist plus the opaque bearer string
/// handed to the client exactly once.
///
/// Only [`Self::token_hash`] is persisted; the secret is never stored, so a theft
/// of the `tokens` table yields no usable bearer tokens.
pub struct NewToken {
    /// Stable, non-secret row id and revocation handle.
    pub token_id: String,
    /// `blake3(secret)` — the only secret-derived value persisted.
    pub token_hash: [u8; 32],
    /// The opaque bearer string `"{token_id}.{base64url(secret)}"`.
    pub bearer: String,
}

impl NewToken {
    /// Generate a new token id + high-entropy secret and derive its bearer
    /// string and stored hash.
    #[must_use]
    pub fn generate() -> Self {
        let token_id = Uuid::new_v4().simple().to_string();
        let mut secret = [0u8; TOKEN_SECRET_LEN];
        OsRng.fill_bytes(&mut secret);
        let token_hash = hash_secret(&secret);
        let bearer = format!("{token_id}.{}", URL_SAFE_NO_PAD.encode(secret));
        Self {
            token_id,
            token_hash,
            bearer,
        }
    }
}

/// `blake3(secret)` — the value persisted for a token so the raw secret never
/// touches disk.
#[must_use]
pub fn hash_secret(secret: &[u8]) -> [u8; 32] {
    *blake3::hash(secret).as_bytes()
}

/// Split an opaque bearer string `"{token_id}.{base64url(secret)}"` into its
/// non-secret id and the decoded secret bytes.
///
/// Returns `None` if the shape is wrong or the secret does not base64url-decode
/// to exactly [`TOKEN_SECRET_LEN`] bytes.
#[must_use]
pub fn parse_bearer(bearer: &str) -> Option<(String, Vec<u8>)> {
    let (token_id, secret_b64) = bearer.split_once('.')?;
    if token_id.is_empty() {
        return None;
    }
    let secret = URL_SAFE_NO_PAD.decode(secret_b64.as_bytes()).ok()?;
    if secret.len() != TOKEN_SECRET_LEN {
        return None;
    }
    Some((token_id.to_string(), secret))
}

/// Constant-time equality for two byte slices (used to compare token hashes so a
/// timing side-channel cannot recover a valid hash byte-by-byte).
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Derive the opaque `device_id` handle from an Ed25519 public key: the first
/// [`DEVICE_ID_LEN`] bytes of its BLAKE3 hash, lowercase hex.
///
/// This deliberately **replicates** `pergamon_crypto::device::
/// device_id_from_ed25519` byte-for-byte so the AGPL server does not link the
/// Apache client crypto at runtime (ADR-008). The parity is asserted in this
/// module's unit tests against the real crypto crate (a dev-dependency).
#[must_use]
pub fn device_id_from_ed25519(ed25519_pub: &[u8; ED25519_PUB_LEN]) -> String {
    let digest = blake3::hash(ed25519_pub);
    let bytes = digest.as_bytes();
    let mut s = String::with_capacity(DEVICE_ID_LEN * 2);
    for b in &bytes[..DEVICE_ID_LEN] {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    s
}

/// Append a big-endian `u32` length prefix followed by the bytes.
fn push_len_prefixed(buf: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// Build the domain-tagged bytes a device signs to prove possession at token
/// **issuance** (mint).
///
/// `tag || len(login_id)||login_id || blake3(KE3) || len(device_id)||device_id ||
/// ed25519_pub`. Binding to the server-generated, single-use `login_id` (and the
/// exact KE3 transcript) makes a captured `PoP` non-replayable into a different
/// login.
#[must_use]
pub fn mint_pop_message(
    login_id: &str,
    credential_finalization: &[u8],
    device_id: &str,
    ed25519_pub: &[u8; ED25519_PUB_LEN],
) -> Vec<u8> {
    let ke3_hash = blake3::hash(credential_finalization);
    let mut msg = Vec::with_capacity(MINT_POP_TAG.len() + 64 + login_id.len() + device_id.len());
    msg.extend_from_slice(MINT_POP_TAG);
    push_len_prefixed(&mut msg, login_id.as_bytes());
    msg.extend_from_slice(ke3_hash.as_bytes());
    push_len_prefixed(&mut msg, device_id.as_bytes());
    msg.extend_from_slice(ed25519_pub);
    msg
}

/// Build the domain-tagged bytes a device signs to prove possession at token
/// **refresh**.
///
/// `tag || len(refresh_token_id)||refresh_token_id || nonce ||
/// len(device_id)||device_id || ed25519_pub`. The client-supplied `nonce` keeps
/// the signature fresh; the refresh-token secret remains the primary gate, and
/// the refresh token is rotated (single-use) on every successful exchange (see
/// [`crate::auth::store::AuthStore::rotate_refresh`]). Server-side nonce
/// tracking and refresh-token *reuse detection* (family revocation on replay of
/// an already-rotated token) are noted hardening seams for external review.
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

/// Verify an Ed25519 signature over `msg` against `ed25519_pub`.
///
/// Uses strict verification, rejecting non-canonical / small-order keys and
/// signatures. Honest ADR-024 device keys (OS-CSPRNG generated) always pass.
#[must_use]
pub fn verify_ed25519(
    ed25519_pub: &[u8; ED25519_PUB_LEN],
    msg: &[u8],
    signature: &[u8; ED25519_SIG_LEN],
) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(ed25519_pub) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    vk.verify_strict(msg, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn bearer_round_trips_and_hash_matches() {
        let t = NewToken::generate();
        let (id, secret) = parse_bearer(&t.bearer).unwrap();
        assert_eq!(id, t.token_id);
        assert_eq!(secret.len(), TOKEN_SECRET_LEN);
        assert_eq!(hash_secret(&secret), t.token_hash);
    }

    #[test]
    fn parse_bearer_rejects_malformed() {
        assert!(parse_bearer("no-dot").is_none());
        assert!(parse_bearer(".onlysecret").is_none());
        assert!(parse_bearer("id.").is_none());
        assert!(parse_bearer("id.!!!not-base64!!!").is_none());
        // Right shape but wrong secret length.
        let short = URL_SAFE_NO_PAD.encode([0u8; 8]);
        assert!(parse_bearer(&format!("id.{short}")).is_none());
    }

    #[test]
    fn constant_time_eq_behaves_like_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn device_id_derivation_matches_pergamon_crypto() {
        // Parity guard: our server-side replica must equal the Apache crypto
        // crate byte-for-byte (it is a dev-dependency, used only here in tests).
        let kp = pergamon_crypto::device::DeviceKeypairs::generate().unwrap();
        let pubkey = kp.ed25519_verifying();
        assert_eq!(
            device_id_from_ed25519(pubkey),
            pergamon_crypto::device::device_id_from_ed25519(pubkey),
        );
        assert_eq!(device_id_from_ed25519(pubkey), kp.device_id());
    }

    #[test]
    fn mint_pop_verifies_with_device_key_and_rejects_tamper() {
        let kp = pergamon_crypto::device::DeviceKeypairs::generate().unwrap();
        let pubkey = kp.ed25519_verifying();
        let device_id = kp.device_id();
        let msg = mint_pop_message("login-123", b"ke3-bytes", device_id, pubkey);
        let sig = kp.sign(&msg);
        assert!(verify_ed25519(pubkey, &msg, &sig));

        // A different login_id yields a different message → same signature fails.
        let other = mint_pop_message("login-999", b"ke3-bytes", device_id, pubkey);
        assert!(!verify_ed25519(pubkey, &other, &sig));
    }

    #[test]
    fn refresh_pop_verifies_with_device_key() {
        let kp = pergamon_crypto::device::DeviceKeypairs::generate().unwrap();
        let pubkey = kp.ed25519_verifying();
        let msg = refresh_pop_message("rt-1", b"nonce-abc", kp.device_id(), pubkey);
        let sig = kp.sign(&msg);
        assert!(verify_ed25519(pubkey, &msg, &sig));
    }

    #[test]
    fn token_config_defaults_are_short_access_long_refresh() {
        let cfg = TokenConfig::default();
        assert_eq!(cfg.access_ttl_ms, 60 * 60 * 1000);
        assert!(cfg.refresh_ttl_ms > cfg.access_ttl_ms);
    }
}
