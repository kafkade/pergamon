// SPDX-License-Identifier: Apache-2.0

//! Convergent (content-derived) encryption of immutable **blobs**.
//!
//! ADR-022 addresses blobs by the hash of their *ciphertext* and deduplicates
//! on it, so identical plaintext must encrypt to identical ciphertext. ADR-024
//! achieves this with convergent encryption: the key **and** the nonce are
//! derived from `BLAKE3(plaintext)` under the per-epoch account content key, so
//! encryption is deterministic within an epoch.
//!
//! Because the key is derived from the plaintext hash, decryption needs that
//! hash — it is *not* recoverable from the ciphertext alone. The producing
//! client therefore records the [`EncryptedBlob::plaintext_hash`] inside the
//! (encrypted) event body that references the blob; the consuming client reads
//! it back after decrypting the event and passes it to [`decrypt_blob`].

use sha2::{Digest, Sha256};

use crate::error::{CryptoError, Result};
use crate::hierarchy::AccountContentKey;
use crate::primitives::{self, NONCE_LEN};

/// HKDF label deriving the convergent blob nonce from the plaintext hash.
const LABEL_BLOB_NONCE: &[u8] = b"pergamon/v1/blob-nonce";
/// Domain tag prefixed to the blob AAD.
const BLOB_AAD_TAG: &[u8] = b"pergamon/v1/blob-aad";

/// The result of encrypting a blob: the opaque ciphertext plus the two hashes a
/// client needs.
#[derive(Debug, Clone)]
pub struct EncryptedBlob {
    /// Content address for the blob store: lowercase-hex SHA-256 of the
    /// **ciphertext** (ADR-022 `ct_hash`). Goes in `blob_refs` and the upload
    /// URL.
    pub ct_hash: String,
    /// BLAKE3 of the **plaintext**: the convergent-key input the consumer needs
    /// to decrypt. The producer stores this inside the encrypted event body.
    pub plaintext_hash: [u8; 32],
    /// The opaque ciphertext bytes (`ciphertext ‖ tag`) uploaded to the server.
    pub ciphertext: Vec<u8>,
}

/// Encrypt an immutable blob convergently under the epoch's content key.
///
/// Deterministic within an epoch: the same `plaintext` yields byte-identical
/// `ciphertext` and thus an identical `ct_hash`, so the server dedups it.
///
/// # Errors
/// Returns [`CryptoError::Malformed`] if the AEAD backend rejects the inputs, or
/// a key-derivation error (never in practice).
pub fn encrypt_blob(ack: &AccountContentKey, plaintext: &[u8]) -> Result<EncryptedBlob> {
    let plaintext_hash = primitives::blake3_hash(plaintext);
    let key = ack.blob_key(&plaintext_hash)?;
    let nonce = blob_nonce(ack, &plaintext_hash)?;
    let aad = blob_aad(ack.epoch(), &plaintext_hash);
    let ciphertext = primitives::aead_encrypt_with_nonce(&key, &nonce, &aad, plaintext)?;
    let ct_hash = sha256_hex(&ciphertext);
    Ok(EncryptedBlob {
        ct_hash,
        plaintext_hash,
        ciphertext,
    })
}

/// Decrypt a blob given the `plaintext_hash` recorded by the producer.
///
/// After decryption the plaintext is re-hashed and checked against
/// `plaintext_hash`, so a substituted ciphertext or a wrong hash is rejected.
///
/// # Errors
/// Returns [`CryptoError::Decryption`] if authentication fails or the recovered
/// plaintext does not match `plaintext_hash`.
pub fn decrypt_blob(
    ack: &AccountContentKey,
    plaintext_hash: &[u8; 32],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let key = ack.blob_key(plaintext_hash)?;
    let nonce = blob_nonce(ack, plaintext_hash)?;
    let aad = blob_aad(ack.epoch(), plaintext_hash);
    let plaintext = primitives::aead_decrypt_with_nonce(&key, &nonce, &aad, ciphertext)?;
    if primitives::blake3_hash(&plaintext) != *plaintext_hash {
        return Err(CryptoError::Decryption);
    }
    Ok(plaintext)
}

/// Compute the ADR-022 content address of some bytes: lowercase-hex SHA-256.
///
/// Identical to the sync server's `ct_hash`, so a client and the server agree on
/// a blob's address without the client depending on the AGPL server crate.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    out
}

/// Derive the deterministic 24-byte convergent nonce for a blob.
fn blob_nonce(ack: &AccountContentKey, plaintext_hash: &[u8; 32]) -> Result<[u8; NONCE_LEN]> {
    let mut info = Vec::with_capacity(LABEL_BLOB_NONCE.len() + plaintext_hash.len());
    info.extend_from_slice(LABEL_BLOB_NONCE);
    info.extend_from_slice(plaintext_hash);
    primitives::hkdf_sha256::<NONCE_LEN>(ack.expose_bytes(), &info)
}

/// Build the blob AEAD associated data, binding the epoch and the plaintext hash.
fn blob_aad(epoch: u32, plaintext_hash: &[u8; 32]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(BLOB_AAD_TAG.len() + 4 + plaintext_hash.len());
    aad.extend_from_slice(BLOB_AAD_TAG);
    aad.extend_from_slice(&epoch.to_be_bytes());
    aad.extend_from_slice(plaintext_hash);
    aad
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::hierarchy::AccountRootKey;

    #[test]
    fn blob_roundtrip() {
        let ark = AccountRootKey::from_bytes([11u8; 32]);
        let ack = ark.content_key(0).unwrap();
        let blob = encrypt_blob(&ack, b"a pdf's bytes").unwrap();
        let pt = decrypt_blob(&ack, &blob.plaintext_hash, &blob.ciphertext).unwrap();
        assert_eq!(pt, b"a pdf's bytes");
        assert!(!blob.ciphertext.windows(13).any(|w| w == b"a pdf's bytes"));
    }

    #[test]
    fn convergent_same_plaintext_same_ciphertext() {
        let ark = AccountRootKey::from_bytes([12u8; 32]);
        let ack = ark.content_key(0).unwrap();
        let a = encrypt_blob(&ack, b"identical").unwrap();
        let b = encrypt_blob(&ack, b"identical").unwrap();
        assert_eq!(a.ciphertext, b.ciphertext, "must be deterministic");
        assert_eq!(a.ct_hash, b.ct_hash, "same address ⇒ dedups");
    }

    #[test]
    fn different_epochs_reencrypt_to_new_blob() {
        let ark = AccountRootKey::from_bytes([12u8; 32]);
        let ack0 = ark.content_key(0).unwrap();
        let ack1 = ark.content_key(1).unwrap();
        let a = encrypt_blob(&ack0, b"same plaintext").unwrap();
        let b = encrypt_blob(&ack1, b"same plaintext").unwrap();
        assert_ne!(a.ct_hash, b.ct_hash);
    }

    #[test]
    fn ct_hash_matches_sha256_of_ciphertext() {
        let ark = AccountRootKey::from_bytes([13u8; 32]);
        let ack = ark.content_key(0).unwrap();
        let blob = encrypt_blob(&ack, b"bytes").unwrap();
        assert_eq!(blob.ct_hash, sha256_hex(&blob.ciphertext));
    }

    #[test]
    fn wrong_plaintext_hash_is_rejected() {
        let ark = AccountRootKey::from_bytes([14u8; 32]);
        let ack = ark.content_key(0).unwrap();
        let blob = encrypt_blob(&ack, b"payload").unwrap();
        let wrong = [0u8; 32];
        assert!(decrypt_blob(&ack, &wrong, &blob.ciphertext).is_err());
    }
}
