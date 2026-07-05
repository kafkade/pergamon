// SPDX-License-Identifier: Apache-2.0

//! The ADR-024 key hierarchy: everything derived from one Account Root Key.
//!
//! ```text
//! Account Root Key (ARK, 256-bit random, never leaves a device in plaintext)
//! ├── account_stream_key = HKDF(ARK, "pergamon/v1/entity-ref")
//! ├── ACK_e              = HKDF(ARK, "pergamon/v1/account-content" ‖ e)
//! │   ├── event key      = HKDF(ACK_e, "pergamon/v1/event" ‖ change_id)
//! │   └── blob key       = HKDF(ACK_e, "pergamon/v1/blob" ‖ BLAKE3(plaintext))
//! └── recovery is a *wrapping* of the ARK (see `recovery`), not a branch
//! ```
//!
//! Every arrow is HKDF-SHA-256 with a distinct, hard-coded `info` label, so no
//! two purposes ever share key material. The `account_id` is deliberately
//! **not** part of this tree: it is an independent random handle so the server
//! identifier leaks nothing about the keys.
//!
//! All derivations are pure and deterministic; only [`AccountRootKey::generate`]
//! and [`AccountId::generate`] draw on the CSPRNG.

use zeroize::Zeroizing;

use crate::error::Result;
use crate::primitives::{self, KEY_LEN, SymmetricKey};

/// Length in bytes of the opaque `account_id` handle (128-bit).
pub const ACCOUNT_ID_LEN: usize = 16;

/// HKDF label deriving the `account_stream_key` used for `entity_ref` blinding.
const LABEL_ENTITY_REF: &[u8] = b"pergamon/v1/entity-ref";
/// HKDF label prefix deriving a per-epoch account content key (`ACK_e`).
const LABEL_ACCOUNT_CONTENT: &[u8] = b"pergamon/v1/account-content";
/// HKDF label prefix deriving a per-event key from an `ACK_e`.
const LABEL_EVENT: &[u8] = b"pergamon/v1/event";
/// HKDF label prefix deriving a convergent blob key from an `ACK_e`.
const LABEL_BLOB: &[u8] = b"pergamon/v1/blob";

/// The **Account Root Key** — the 256-bit secret at the root of the hierarchy.
///
/// The ARK is the only thing enrollment and recovery transfer. It never leaves
/// a device except wrapped as ciphertext. Held in a zeroizing buffer so it is
/// wiped from memory on drop.
#[derive(Clone)]
pub struct AccountRootKey(SymmetricKey);

impl AccountRootKey {
    /// Generate a fresh random ARK for a brand-new account.
    ///
    /// # Errors
    /// Returns [`crate::CryptoError::Random`] if the OS CSPRNG fails.
    pub fn generate() -> Result<Self> {
        Ok(Self(primitives::random_key()?))
    }

    /// Reconstruct an ARK from its 32 raw bytes (e.g. after unwrapping an
    /// enrollment bundle or recovery blob).
    #[must_use]
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Borrow the ARK's raw bytes, e.g. to seal it into an enrollment bundle or
    /// wrap it for recovery. Handle with care; never send in plaintext.
    #[must_use]
    pub fn expose_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Derive the `account_stream_key`, the stable HMAC key that ADR-022's
    /// `entity_ref` blinding uses so the same entity always blinds identically.
    ///
    /// # Errors
    /// Returns an error if key derivation fails (never in practice).
    pub fn account_stream_key(&self) -> Result<SymmetricKey> {
        primitives::hkdf_key(self.0.as_slice(), LABEL_ENTITY_REF)
    }

    /// Derive the account content key for a given `key_epoch` (`ACK_e`).
    ///
    /// A device holding the ARK can derive every epoch's key, so it can read
    /// history across rotations. The label is `"pergamon/v1/account-content"`
    /// concatenated with the epoch as big-endian bytes.
    ///
    /// # Errors
    /// Returns an error if key derivation fails (never in practice).
    pub fn content_key(&self, epoch: u32) -> Result<AccountContentKey> {
        let mut info = Vec::with_capacity(LABEL_ACCOUNT_CONTENT.len() + 4);
        info.extend_from_slice(LABEL_ACCOUNT_CONTENT);
        info.extend_from_slice(&epoch.to_be_bytes());
        let key = primitives::hkdf_key(self.0.as_slice(), &info)?;
        Ok(AccountContentKey { epoch, key })
    }
}

/// A per-epoch **account content key** (`ACK_e`): the KEK/derivation root that
/// encrypts ADR-022 events and blobs for one `key_epoch`.
#[derive(Clone)]
pub struct AccountContentKey {
    epoch: u32,
    key: SymmetricKey,
}

impl AccountContentKey {
    /// The `key_epoch` this content key belongs to (the ADR-022 envelope field).
    #[must_use]
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Borrow the raw content-key bytes (e.g. to seal an epoch-key set into an
    /// enrollment bundle or rotation re-wrap).
    #[must_use]
    pub fn expose_bytes(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    /// Reconstruct a content key for `epoch` from its raw bytes (after
    /// unwrapping an enrollment bundle or rotation re-wrap).
    #[must_use]
    pub fn from_bytes(epoch: u32, bytes: [u8; KEY_LEN]) -> Self {
        Self {
            epoch,
            key: Zeroizing::new(bytes),
        }
    }

    /// Derive the per-event key for a given `change_id`.
    ///
    /// Deterministic (any holder of this `ACK_e` reproduces it) yet unique per
    /// event, so event bodies never share a key.
    ///
    /// # Errors
    /// Returns an error if key derivation fails (never in practice).
    pub fn event_key(&self, change_id: &str) -> Result<SymmetricKey> {
        let mut info = Vec::with_capacity(LABEL_EVENT.len() + change_id.len());
        info.extend_from_slice(LABEL_EVENT);
        info.extend_from_slice(change_id.as_bytes());
        primitives::hkdf_key(self.key.as_slice(), &info)
    }

    /// Derive the convergent blob key from the BLAKE3 hash of the *plaintext*.
    ///
    /// Identical plaintext under the same epoch yields an identical key (and, in
    /// [`crate::blob`], an identical content-derived nonce), so encryption is
    /// deterministic and ADR-022's ciphertext-hash blob dedup keeps working.
    ///
    /// # Errors
    /// Returns an error if key derivation fails (never in practice).
    pub fn blob_key(&self, plaintext_hash: &[u8; 32]) -> Result<SymmetricKey> {
        let mut info = Vec::with_capacity(LABEL_BLOB.len() + plaintext_hash.len());
        info.extend_from_slice(LABEL_BLOB);
        info.extend_from_slice(plaintext_hash);
        primitives::hkdf_key(self.key.as_slice(), &info)
    }
}

/// The opaque, server-visible **account handle** (ADR-022 `account_id`).
///
/// A 128-bit random value, independent of the ARK, so the identifier the server
/// indexes reveals nothing about the key material. Rendered as lowercase hex on
/// the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountId([u8; ACCOUNT_ID_LEN]);

impl AccountId {
    /// Generate a fresh random account handle.
    ///
    /// # Errors
    /// Returns [`crate::CryptoError::Random`] if the OS CSPRNG fails.
    pub fn generate() -> Result<Self> {
        Ok(Self(primitives::random_array::<ACCOUNT_ID_LEN>()?))
    }

    /// Wrap raw handle bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; ACCOUNT_ID_LEN]) -> Self {
        Self(bytes)
    }

    /// The raw handle bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; ACCOUNT_ID_LEN] {
        &self.0
    }

    /// Render the handle as a 32-character lowercase-hex string (the ADR-022
    /// `account_id` wire form).
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(ACCOUNT_ID_LEN * 2);
        for byte in &self.0 {
            // Writing a hex byte to a String is infallible.
            s.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            s.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn ark_derivations_are_deterministic() {
        let ark = AccountRootKey::from_bytes([1u8; 32]);
        let stream_first = ark.account_stream_key().unwrap();
        let stream_second = ark.account_stream_key().unwrap();
        assert_eq!(*stream_first, *stream_second);

        let content_first = ark.content_key(0).unwrap();
        let content_second = ark.content_key(0).unwrap();
        assert_eq!(content_first.expose_bytes(), content_second.expose_bytes());
    }

    #[test]
    fn epochs_derive_distinct_content_keys() {
        let ark = AccountRootKey::from_bytes([2u8; 32]);
        let ack0 = ark.content_key(0).unwrap();
        let ack1 = ark.content_key(1).unwrap();
        assert_ne!(ack0.expose_bytes(), ack1.expose_bytes());
        assert_eq!(ack0.epoch(), 0);
        assert_eq!(ack1.epoch(), 1);
    }

    #[test]
    fn stream_key_and_content_key_are_separated() {
        let ark = AccountRootKey::from_bytes([3u8; 32]);
        let ask = ark.account_stream_key().unwrap();
        let ack0 = ark.content_key(0).unwrap();
        assert_ne!(ask.as_slice(), ack0.expose_bytes().as_slice());
    }

    #[test]
    fn event_keys_are_unique_and_deterministic() {
        let ark = AccountRootKey::from_bytes([4u8; 32]);
        let ack = ark.content_key(0).unwrap();
        let a = ack.event_key("change-a").unwrap();
        let a2 = ack.event_key("change-a").unwrap();
        let b = ack.event_key("change-b").unwrap();
        assert_eq!(*a, *a2);
        assert_ne!(*a, *b);
    }

    #[test]
    fn blob_keys_are_convergent_per_epoch() {
        let ark = AccountRootKey::from_bytes([5u8; 32]);
        let ack0 = ark.content_key(0).unwrap();
        let ack1 = ark.content_key(1).unwrap();
        let h = primitives::blake3_hash(b"plaintext");
        // Same epoch + same plaintext => same key (convergent).
        assert_eq!(*ack0.blob_key(&h).unwrap(), *ack0.blob_key(&h).unwrap());
        // Different epoch => different key (re-encrypts to a new blob).
        assert_ne!(*ack0.blob_key(&h).unwrap(), *ack1.blob_key(&h).unwrap());
    }

    #[test]
    fn account_id_hex_roundtrips_shape() {
        let id = AccountId::from_bytes([0xab; ACCOUNT_ID_LEN]);
        let hex = id.to_hex();
        assert_eq!(hex.len(), ACCOUNT_ID_LEN * 2);
        assert_eq!(hex, "abababababababababababababababab");
    }

    #[test]
    fn generated_account_ids_differ() {
        let a = AccountId::generate().unwrap();
        let b = AccountId::generate().unwrap();
        assert_ne!(a, b);
    }
}
