// SPDX-License-Identifier: Apache-2.0

//! Authenticated encryption of ADR-022 **event bodies**, plus `entity_ref`
//! blinding.
//!
//! The server-visible header (`protocol_version, account_id, change_id,
//! key_epoch, blob_refs`) is bound into the AEAD as associated data (AAD), so a
//! server cannot re-target, re-epoch, or replay a body under a different header
//! without the authentication failing. The body itself — entity type/id, op,
//! clock, fields — lives only inside the ciphertext and is never seen here or by
//! the server.

use serde::{Deserialize, Serialize};

use crate::error::{CryptoError, Result};
use crate::hierarchy::AccountContentKey;
use crate::primitives;

/// Domain tag prefixed to the event AAD so it can never collide with any other
/// authenticated context.
const EVENT_AAD_TAG: &[u8] = b"pergamon/v1/event-aad";
/// Domain tag prefixed to the `entity_ref` HMAC input.
const ENTITY_REF_TAG: &[u8] = b"pergamon/v1/entity-ref-input";

/// The server-visible header fields that ADR-022 requires to be bound as AEAD
/// associated data.
///
/// This mirrors the ADR-022 `EventInput` frame minus the ciphertext itself. A
/// client builds it for the event it is about to encrypt; the exact same header
/// must be presented to decrypt, which is what binds a body to its routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventHeader {
    /// Wire protocol major version the client is speaking.
    pub protocol_version: u32,
    /// Opaque account handle (ADR-022 `account_id`, hex).
    pub account_id: String,
    /// Client-generated globally unique idempotency key.
    pub change_id: String,
    /// Account key epoch that encrypts this body (must match the `ACK_e` used).
    pub key_epoch: u32,
    /// Ciphertext hashes of blobs this event depends on.
    pub blob_refs: Vec<String>,
}

impl EventHeader {
    /// Serialize the header into canonical, unambiguous AAD bytes.
    ///
    /// Every variable-length field is length-prefixed (big-endian `u32`) so no
    /// two distinct headers can ever produce the same byte string.
    #[must_use]
    pub fn aad_bytes(&self) -> Vec<u8> {
        let mut aad = Vec::new();
        aad.extend_from_slice(EVENT_AAD_TAG);
        aad.extend_from_slice(&self.protocol_version.to_be_bytes());
        push_lp(&mut aad, self.account_id.as_bytes());
        push_lp(&mut aad, self.change_id.as_bytes());
        aad.extend_from_slice(&self.key_epoch.to_be_bytes());
        aad.extend_from_slice(&u32_len(self.blob_refs.len()).to_be_bytes());
        for r in &self.blob_refs {
            push_lp(&mut aad, r.as_bytes());
        }
        aad
    }
}

/// Encrypt an ADR-022 event body under the account content key for its epoch.
///
/// The returned buffer is the opaque `ciphertext_b64` payload's raw bytes
/// (`nonce ‖ ciphertext ‖ tag`); the caller base64-encodes it for the wire.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if `header.key_epoch` does not match the
/// supplied `ack`, or if encryption fails.
pub fn encrypt_event(
    ack: &AccountContentKey,
    header: &EventHeader,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    if header.key_epoch != ack.epoch() {
        return Err(CryptoError::Malformed(
            "event header key_epoch does not match content key epoch",
        ));
    }
    let key = ack.event_key(&header.change_id)?;
    primitives::aead_seal(&key, &header.aad_bytes(), plaintext)
}

/// Decrypt an ADR-022 event body, re-binding the header as AAD.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if the epoch mismatches, or
/// [`CryptoError::Decryption`] if the header, key, or ciphertext do not match
/// what produced it.
pub fn decrypt_event(
    ack: &AccountContentKey,
    header: &EventHeader,
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    if header.key_epoch != ack.epoch() {
        return Err(CryptoError::Malformed(
            "event header key_epoch does not match content key epoch",
        ));
    }
    let key = ack.event_key(&header.change_id)?;
    primitives::aead_open(&key, &header.aad_bytes(), ciphertext)
}

/// Blind an entity reference into the opaque ADR-022 `entity_ref` grouping
/// token.
///
/// Computes `HMAC-SHA-256(account_stream_key, entity_type ‖ entity_id)` (with a
/// domain tag and length-prefixing so the two parts can't be confused) and
/// renders it lowercase hex. Because the stream key is stable per account, the
/// same entity always blinds to the same token, which is what lets the server
/// coalesce per-entity without learning identity.
///
/// # Errors
/// Returns [`CryptoError::KeyDerivation`] only if the HMAC backend rejects the
/// key length (impossible for SHA-256).
pub fn entity_ref(
    account_stream_key: &[u8; primitives::KEY_LEN],
    entity_type: &str,
    entity_id: &str,
) -> Result<String> {
    let mut msg = Vec::new();
    msg.extend_from_slice(ENTITY_REF_TAG);
    push_lp(&mut msg, entity_type.as_bytes());
    push_lp(&mut msg, entity_id.as_bytes());
    let tag = primitives::hmac_sha256(account_stream_key, &msg)?;
    Ok(to_hex(&tag))
}

/// Append a length-prefixed byte string (big-endian `u32` length then bytes).
fn push_lp(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&u32_len(bytes.len()).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// Saturating conversion of a length to `u32` for the wire framing.
fn u32_len(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// Render bytes as a lowercase-hex string.
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::hierarchy::AccountRootKey;

    fn header() -> EventHeader {
        EventHeader {
            protocol_version: 1,
            account_id: "abababababababababababababababab".to_owned(),
            change_id: "change-0001".to_owned(),
            key_epoch: 0,
            blob_refs: vec!["hashA".to_owned(), "hashB".to_owned()],
        }
    }

    #[test]
    fn event_roundtrip() {
        let ark = AccountRootKey::from_bytes([9u8; 32]);
        let ack = ark.content_key(0).unwrap();
        let h = header();
        let ct = encrypt_event(&ack, &h, b"secret body").unwrap();
        let pt = decrypt_event(&ack, &h, &ct).unwrap();
        assert_eq!(pt, b"secret body");
        // Ciphertext must not contain the plaintext.
        assert!(!ct.windows(11).any(|w| w == b"secret body"));
    }

    #[test]
    fn tampered_header_fails_decryption() {
        let ark = AccountRootKey::from_bytes([9u8; 32]);
        let ack = ark.content_key(0).unwrap();
        let h = header();
        let ct = encrypt_event(&ack, &h, b"body").unwrap();

        let mut wrong = header();
        wrong.blob_refs = vec!["hashA".to_owned()];
        assert!(matches!(
            decrypt_event(&ack, &wrong, &ct),
            Err(CryptoError::Decryption)
        ));
    }

    #[test]
    fn epoch_mismatch_is_rejected() {
        let ark = AccountRootKey::from_bytes([9u8; 32]);
        let ack1 = ark.content_key(1).unwrap();
        let h = header(); // key_epoch = 0
        assert!(matches!(
            encrypt_event(&ack1, &h, b"body"),
            Err(CryptoError::Malformed(_))
        ));
    }

    #[test]
    fn wrong_epoch_key_cannot_decrypt() {
        let ark = AccountRootKey::from_bytes([9u8; 32]);
        let ack0 = ark.content_key(0).unwrap();
        let h = header();
        let ct = encrypt_event(&ack0, &h, b"body").unwrap();
        // A header claiming epoch 1 paired with ACK_1 derives a different key.
        let ack1 = ark.content_key(1).unwrap();
        let mut h1 = header();
        h1.key_epoch = 1;
        assert!(decrypt_event(&ack1, &h1, &ct).is_err());
    }

    #[test]
    fn entity_ref_is_stable_and_separated() {
        let ark = AccountRootKey::from_bytes([1u8; 32]);
        let ask = ark.account_stream_key().unwrap();
        let a = entity_ref(&ask, "document", "doc-1").unwrap();
        let a2 = entity_ref(&ask, "document", "doc-1").unwrap();
        let b = entity_ref(&ask, "document", "doc-2").unwrap();
        let c = entity_ref(&ask, "highlight", "doc-1").unwrap();
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn entity_ref_length_prefix_prevents_ambiguity() {
        let ark = AccountRootKey::from_bytes([1u8; 32]);
        let ask = ark.account_stream_key().unwrap();
        // Without length-prefixing, ("ab","c") and ("a","bc") would collide.
        assert_ne!(
            entity_ref(&ask, "ab", "c").unwrap(),
            entity_ref(&ask, "a", "bc").unwrap()
        );
    }
}
