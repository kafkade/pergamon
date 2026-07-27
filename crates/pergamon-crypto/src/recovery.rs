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
use crate::hierarchy::{ACCOUNT_ID_LEN, AccountId, AccountRootKey};
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

/// Magic identifying a serialized [`KeyPackage`] (8 bytes; the trailing
/// `\x01\x00` are a fixed part of the identifier, not the container version —
/// see [`KEY_PACKAGE_VERSION`]).
pub const KEY_PACKAGE_MAGIC: [u8; 8] = *b"PGMKEY\x01\x00";

/// Container-layout version of a serialized [`KeyPackage`]. Bump when the byte
/// layout after the magic changes (a new KDF/AEAD, extra fields, …).
pub const KEY_PACKAGE_VERSION: u8 = 1;

/// Byte offset of the account handle within a serialized key package.
const KEY_PACKAGE_ACCOUNT_OFFSET: usize = KEY_PACKAGE_MAGIC.len() + 1;
/// Byte offset of the wrapped recovery blob within a serialized key package.
const KEY_PACKAGE_BLOB_OFFSET: usize = KEY_PACKAGE_ACCOUNT_OFFSET + ACCOUNT_ID_LEN;

/// A self-contained, passphrase-protected **key package**: the account handle
/// plus a [`RecoveryBlob`] wrapping the Account Root Key.
///
/// A plaintext `export backup` archive deliberately contains **no key
/// material**, so on its own it cannot restore an encrypted / sync-enabled
/// account. A key package is the missing half: exported once, kept somewhere
/// safe, it lets a client reconstruct the ARK (and therefore every derived key)
/// from the file plus its passphrase alone — no other enrolled device required.
///
/// The serialized form embeds both the `account_id` and the recovery blob so
/// import needs only the file and the passphrase. Anyone holding both the file
/// and the passphrase gains full account access; treat it like a master key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPackage {
    /// The opaque account handle the wrapped ARK belongs to.
    pub account_id: AccountId,
    /// The Argon2id-wrapped Account Root Key.
    pub blob: RecoveryBlob,
}

impl KeyPackage {
    /// Serialize to self-describing bytes:
    /// `MAGIC(8) ‖ version(1) ‖ account_id(16) ‖ RecoveryBlob::to_bytes()`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let blob_bytes = self.blob.to_bytes();
        let mut out = Vec::with_capacity(KEY_PACKAGE_BLOB_OFFSET + blob_bytes.len());
        out.extend_from_slice(&KEY_PACKAGE_MAGIC);
        out.push(KEY_PACKAGE_VERSION);
        out.extend_from_slice(self.account_id.as_bytes());
        out.extend_from_slice(&blob_bytes);
        out
    }

    /// Parse a key package from its self-describing serialized form.
    ///
    /// # Errors
    /// [`CryptoError::Malformed`] if the magic or version does not match, or the
    /// input is too short to hold the account handle and a recovery blob.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < KEY_PACKAGE_BLOB_OFFSET {
            return Err(CryptoError::Malformed("key package too short"));
        }
        if bytes[..KEY_PACKAGE_MAGIC.len()] != KEY_PACKAGE_MAGIC {
            return Err(CryptoError::Malformed("not a pergamon key package"));
        }
        if bytes[KEY_PACKAGE_MAGIC.len()] != KEY_PACKAGE_VERSION {
            return Err(CryptoError::Malformed("unsupported key package version"));
        }
        let mut id = [0u8; ACCOUNT_ID_LEN];
        id.copy_from_slice(&bytes[KEY_PACKAGE_ACCOUNT_OFFSET..KEY_PACKAGE_BLOB_OFFSET]);
        let blob = RecoveryBlob::from_bytes(&bytes[KEY_PACKAGE_BLOB_OFFSET..])?;
        Ok(Self {
            account_id: AccountId::from_bytes(id),
            blob,
        })
    }
}

/// Build a [`KeyPackage`] wrapping `ark` under a key stretched from `secret`.
///
/// This is [`enable_recovery`] plus the account handle, packaged for portable
/// storage. Feed the same `secret` to [`import_key_package`] to recover.
///
/// # Errors
/// Returns a crypto error if the CSPRNG or key stretching fails.
pub fn export_key_package(
    ark: &AccountRootKey,
    account_id: &AccountId,
    secret: &[u8],
) -> Result<KeyPackage> {
    let blob = enable_recovery(ark, account_id, secret)?;
    Ok(KeyPackage {
        account_id: account_id.clone(),
        blob,
    })
}

/// Recover the Account Root Key from a [`KeyPackage`] and its passphrase.
///
/// # Errors
/// [`CryptoError::Decryption`] if the passphrase is wrong or the package was
/// tampered with; [`CryptoError::Malformed`] if the unwrapped key is not 32
/// bytes.
pub fn import_key_package(package: &KeyPackage, secret: &[u8]) -> Result<AccountRootKey> {
    recover(&package.blob, &package.account_id, secret)
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

    #[test]
    fn key_package_roundtrip() {
        let (ark, id) = account();
        let package = export_key_package(&ark, &id, b"pack passphrase").unwrap();
        let recovered = import_key_package(&package, b"pack passphrase").unwrap();
        assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
    }

    #[test]
    fn key_package_serialization_roundtrips() {
        let (ark, id) = account();
        let package = export_key_package(&ark, &id, b"pw").unwrap();
        let bytes = package.to_bytes();
        assert_eq!(&bytes[..KEY_PACKAGE_MAGIC.len()], &KEY_PACKAGE_MAGIC);
        let parsed = KeyPackage::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, package);
        let recovered = import_key_package(&parsed, b"pw").unwrap();
        assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
    }

    #[test]
    fn key_package_wrong_passphrase_fails() {
        let (ark, id) = account();
        let package = export_key_package(&ark, &id, b"right").unwrap();
        assert!(matches!(
            import_key_package(&package, b"wrong"),
            Err(CryptoError::Decryption)
        ));
    }

    #[test]
    fn key_package_from_bytes_rejects_bad_magic() {
        let (ark, id) = account();
        let mut bytes = export_key_package(&ark, &id, b"pw").unwrap().to_bytes();
        bytes[0] ^= 0xff;
        assert!(matches!(
            KeyPackage::from_bytes(&bytes),
            Err(CryptoError::Malformed(_))
        ));
    }
}
