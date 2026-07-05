// SPDX-License-Identifier: Apache-2.0

//! Optional, opt-in account recovery (ADR-024).
//!
//! Recovery lets an account holder who has lost every enrolled device restore
//! access from a memorized passphrase or a printed high-entropy recovery code.
//! It works by wrapping the Account Root Key under a key stretched from that
//! secret with Argon2id, producing an opaque **recovery blob** the server stores
//! and relays but cannot open.
//!
//! Because the ARK is epoch-independent (rotation only advances the content key),
//! a recovery blob made once stays valid across rotations; the wrap AAD binds
//! only the `account_id`.
//!
//! Recovery is off by default: no blob exists unless the user explicitly enables
//! it, so the offline-guessing surface it introduces is opt-in.

use crate::error::{CryptoError, Result};
use crate::hierarchy::{AccountId, AccountRootKey};
use crate::primitives::{self, KEY_LEN};

/// Length of the per-blob Argon2id salt.
pub const RECOVERY_SALT_LEN: usize = 16;

/// Number of random bytes behind a generated recovery code (≈128-bit).
const RECOVERY_CODE_BYTES: usize = 20;

/// Domain tag prefixed to the recovery-wrap AEAD associated data.
const RECOVERY_AAD_TAG: &[u8] = b"pergamon/v1/recovery-aad";

/// An opaque, server-stored recovery blob: a per-blob salt plus the Argon2id-
/// wrapped Account Root Key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryBlob {
    /// Random per-blob Argon2id salt.
    pub salt: [u8; RECOVERY_SALT_LEN],
    /// `aead_seal` of the ARK under the stretched key-encryption key.
    pub wrapped: Vec<u8>,
}

impl RecoveryBlob {
    /// Serialize to opaque bytes for storage: `salt ‖ wrapped`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(RECOVERY_SALT_LEN + self.wrapped.len());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.wrapped);
        out
    }

    /// Parse a recovery blob from its opaque serialized form.
    ///
    /// # Errors
    /// [`CryptoError::Malformed`] if the input is too short to hold a salt and a
    /// non-empty ciphertext.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() <= RECOVERY_SALT_LEN {
            return Err(CryptoError::Malformed("recovery blob too short"));
        }
        let mut salt = [0u8; RECOVERY_SALT_LEN];
        salt.copy_from_slice(&bytes[..RECOVERY_SALT_LEN]);
        Ok(Self {
            salt,
            wrapped: bytes[RECOVERY_SALT_LEN..].to_vec(),
        })
    }
}

/// Enable recovery: wrap `ark` under a fresh Argon2id key-encryption key derived
/// from `secret` (a passphrase or [`generate_recovery_code`] output).
///
/// # Errors
/// Returns a crypto error if the CSPRNG or key stretching fails.
pub fn enable_recovery(
    ark: &AccountRootKey,
    account_id: &AccountId,
    secret: &[u8],
) -> Result<RecoveryBlob> {
    let salt = primitives::random_array::<RECOVERY_SALT_LEN>()?;
    let kek = primitives::argon2id_kek(secret, &salt)?;
    let wrapped = primitives::aead_seal(&kek, &recovery_aad(account_id), ark.expose_bytes())?;
    Ok(RecoveryBlob { salt, wrapped })
}

/// Recover the Account Root Key from a recovery blob and the account secret.
///
/// # Errors
/// [`CryptoError::Decryption`] if the secret is wrong or the blob was tampered
/// with; [`CryptoError::Malformed`] if the unwrapped plaintext is not a 32-byte
/// key.
pub fn recover(
    blob: &RecoveryBlob,
    account_id: &AccountId,
    secret: &[u8],
) -> Result<AccountRootKey> {
    let kek = primitives::argon2id_kek(secret, &blob.salt)?;
    let plaintext = primitives::aead_open(&kek, &recovery_aad(account_id), &blob.wrapped)?;
    let bytes: [u8; KEY_LEN] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::Malformed("recovered key has wrong length"))?;
    Ok(AccountRootKey::from_bytes(bytes))
}

/// Generate a fresh, high-entropy printable recovery code (Crockford base32,
/// grouped in fours). Suitable to hand to a user to write down; feed it back to
/// [`recover`] as the `secret`.
///
/// # Errors
/// [`CryptoError::Random`] if the OS CSPRNG fails.
pub fn generate_recovery_code() -> Result<String> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let raw = primitives::random_array::<RECOVERY_CODE_BYTES>()?;
    let mut code = String::new();
    for (i, byte) in raw.iter().enumerate() {
        // Map each byte to two base32 symbols (256 -> 2x32 loses 2 bits/byte,
        // still ~120 bits over 20 bytes; ample and simple).
        let hi = usize::from(byte >> 3) & 0x1f;
        let lo = usize::from(byte << 2) & 0x1f;
        code.push(char::from(ALPHABET[hi]));
        code.push(char::from(ALPHABET[lo]));
        if i % 2 == 1 && i + 1 != raw.len() {
            code.push('-');
        }
    }
    Ok(code)
}

/// Build the recovery-wrap AEAD associated data binding the account handle.
fn recovery_aad(account_id: &AccountId) -> Vec<u8> {
    let mut aad = Vec::with_capacity(RECOVERY_AAD_TAG.len() + account_id.as_bytes().len());
    aad.extend_from_slice(RECOVERY_AAD_TAG);
    aad.extend_from_slice(account_id.as_bytes());
    aad
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::hierarchy::ACCOUNT_ID_LEN;

    fn account() -> (AccountRootKey, AccountId) {
        (
            AccountRootKey::from_bytes([42u8; KEY_LEN]),
            AccountId::from_bytes([3u8; ACCOUNT_ID_LEN]),
        )
    }

    #[test]
    fn recovery_roundtrip() {
        let (ark, id) = account();
        let blob = enable_recovery(&ark, &id, b"correct horse battery staple").unwrap();
        let recovered = recover(&blob, &id, b"correct horse battery staple").unwrap();
        assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
    }

    #[test]
    fn wrong_passphrase_fails() {
        let (ark, id) = account();
        let blob = enable_recovery(&ark, &id, b"right").unwrap();
        assert!(matches!(
            recover(&blob, &id, b"wrong"),
            Err(CryptoError::Decryption)
        ));
    }

    #[test]
    fn wrong_account_id_fails_aad() {
        let (ark, id) = account();
        let blob = enable_recovery(&ark, &id, b"pw").unwrap();
        let other = AccountId::from_bytes([9u8; ACCOUNT_ID_LEN]);
        assert!(matches!(
            recover(&blob, &other, b"pw"),
            Err(CryptoError::Decryption)
        ));
    }

    #[test]
    fn blob_serialization_roundtrips() {
        let (ark, id) = account();
        let blob = enable_recovery(&ark, &id, b"pw").unwrap();
        let bytes = blob.to_bytes();
        let parsed = RecoveryBlob::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, blob);
        let recovered = recover(&parsed, &id, b"pw").unwrap();
        assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
    }

    #[test]
    fn generated_code_works_as_secret() {
        let (ark, id) = account();
        let code = generate_recovery_code().unwrap();
        assert!(code.len() > 20);
        let blob = enable_recovery(&ark, &id, code.as_bytes()).unwrap();
        let recovered = recover(&blob, &id, code.as_bytes()).unwrap();
        assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
    }

    #[test]
    fn from_bytes_rejects_short_input() {
        assert!(RecoveryBlob::from_bytes(&[0u8; RECOVERY_SALT_LEN]).is_err());
    }
}
