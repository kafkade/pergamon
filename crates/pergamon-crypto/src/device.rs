// SPDX-License-Identifier: Apache-2.0

//! Per-device keypairs and self-signed device records.
//!
//! On first run each device generates two keypairs (ADR-024):
//!
//! - an **X25519** keypair for key agreement — receiving sealed enrollment and
//!   rotation bundles, and
//! - an **Ed25519** keypair for attestation — signing its own device record and
//!   (once trusted) trust/revocation statements.
//!
//! The private halves live in the platform secure store and never sync; this
//! crate only generates them and operates on them in memory (zeroized on drop).
//! The public `device_id` is derived from the Ed25519 public key so the opaque
//! ADR-022 origin handle is cryptographically bound to the device's signing
//! identity and cannot be claimed by another key.

use crate::error::{CryptoError, Result};
use crate::primitives::{self, KEY_LEN, SIG_LEN, SymmetricKey};

/// Length in bytes of the raw `device_id` handle before hex encoding (128-bit).
pub const DEVICE_ID_LEN: usize = 16;

/// Domain tag prefixed to the bytes a device record signs over.
const DEVICE_RECORD_TAG: &[u8] = b"pergamon/v1/device-record";

/// A device's two keypairs, holding both secret halves (zeroized on drop).
pub struct DeviceKeypairs {
    x25519_secret: SymmetricKey,
    x25519_public: [u8; KEY_LEN],
    ed25519_signing: SymmetricKey,
    ed25519_verifying: [u8; KEY_LEN],
    device_id: String,
}

impl DeviceKeypairs {
    /// Generate a fresh pair of device keypairs.
    ///
    /// # Errors
    /// Returns [`CryptoError::Random`] if the OS CSPRNG fails.
    pub fn generate() -> Result<Self> {
        let (x25519_secret, x25519_public) = primitives::x25519_generate()?;
        let (ed25519_signing, ed25519_verifying) = primitives::ed25519_generate()?;
        let device_id = device_id_from_ed25519(&ed25519_verifying);
        Ok(Self {
            x25519_secret,
            x25519_public,
            ed25519_signing,
            ed25519_verifying,
            device_id,
        })
    }

    /// Reconstruct keypairs from their two stored secret scalars (e.g. after
    /// loading them back from the OS keychain).
    #[must_use]
    pub fn from_secrets(x25519_secret: [u8; KEY_LEN], ed25519_signing: [u8; KEY_LEN]) -> Self {
        let x25519_public = primitives::x25519_public(&x25519_secret);
        let ed25519_verifying = primitives::ed25519_public(&ed25519_signing);
        let device_id = device_id_from_ed25519(&ed25519_verifying);
        Self {
            x25519_secret: zeroize::Zeroizing::new(x25519_secret),
            x25519_public,
            ed25519_signing: zeroize::Zeroizing::new(ed25519_signing),
            ed25519_verifying,
            device_id,
        }
    }

    /// The opaque, key-bound device handle (ADR-022 `device_id`, hex).
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The device's X25519 public key (enrollment/rotation recipient).
    #[must_use]
    pub const fn x25519_public(&self) -> &[u8; KEY_LEN] {
        &self.x25519_public
    }

    /// The device's Ed25519 verifying key.
    #[must_use]
    pub const fn ed25519_verifying(&self) -> &[u8; KEY_LEN] {
        &self.ed25519_verifying
    }

    /// Borrow the X25519 secret scalar (to open sealed boxes addressed to this
    /// device). Handle with care.
    #[must_use]
    pub fn x25519_secret(&self) -> &[u8; KEY_LEN] {
        &self.x25519_secret
    }

    /// Borrow the Ed25519 signing secret (to persist the device identity in the
    /// platform secure store). Handle with care.
    #[must_use]
    pub fn ed25519_signing(&self) -> &[u8; KEY_LEN] {
        &self.ed25519_signing
    }

    /// Sign `msg` with this device's Ed25519 key.
    #[must_use]
    pub fn sign(&self, msg: &[u8]) -> [u8; SIG_LEN] {
        primitives::ed25519_sign(&self.ed25519_signing, msg)
    }

    /// Build and sign this device's self-signed [`SignedDeviceRecord`].
    ///
    /// `created_at` (epoch millis) is supplied by the caller because this crate
    /// is clock-free (ADR-001 zero-I/O).
    #[must_use]
    pub fn sign_record(&self, created_at: i64) -> SignedDeviceRecord {
        let record = DeviceRecord {
            device_id: self.device_id.clone(),
            x25519_pub: self.x25519_public,
            ed25519_pub: self.ed25519_verifying,
            created_at,
        };
        let signature = self.sign(&record.signing_bytes());
        SignedDeviceRecord { record, signature }
    }
}

/// A device's public roster entry: `{device_id, x25519_pub, ed25519_pub,
/// created_at}` (ADR-024). Synced as an ordinary entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    /// The key-bound opaque device handle (hex).
    pub device_id: String,
    /// The device's X25519 public key.
    pub x25519_pub: [u8; KEY_LEN],
    /// The device's Ed25519 verifying key.
    pub ed25519_pub: [u8; KEY_LEN],
    /// Device creation time in epoch milliseconds (supplied by the platform).
    pub created_at: i64,
}

impl DeviceRecord {
    /// Canonical, unambiguous bytes this record is signed over.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(DEVICE_RECORD_TAG);
        let id = self.device_id.as_bytes();
        b.extend_from_slice(&u32_len(id.len()).to_be_bytes());
        b.extend_from_slice(id);
        b.extend_from_slice(&self.x25519_pub);
        b.extend_from_slice(&self.ed25519_pub);
        b.extend_from_slice(&self.created_at.to_be_bytes());
        b
    }
}

/// A [`DeviceRecord`] together with the device's self-signature over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDeviceRecord {
    /// The signed device record.
    pub record: DeviceRecord,
    /// The device's Ed25519 signature over [`DeviceRecord::signing_bytes`].
    pub signature: [u8; SIG_LEN],
}

impl SignedDeviceRecord {
    /// Verify the self-signature and that the `device_id` matches the Ed25519
    /// key (so the opaque handle cannot be forged onto another key).
    ///
    /// # Errors
    /// Returns [`CryptoError::Malformed`] if the `device_id` does not match the
    /// key, or [`CryptoError::BadSignature`] if the signature does not verify.
    pub fn verify(&self) -> Result<()> {
        let expected = device_id_from_ed25519(&self.record.ed25519_pub);
        if expected != self.record.device_id {
            return Err(CryptoError::Malformed(
                "device_id does not match ed25519 public key",
            ));
        }
        primitives::ed25519_verify(
            &self.record.ed25519_pub,
            &self.record.signing_bytes(),
            &self.signature,
        )
    }
}

/// Derive the opaque `device_id` handle from an Ed25519 public key: the first
/// [`DEVICE_ID_LEN`] bytes of its BLAKE3 hash, lowercase hex.
#[must_use]
pub fn device_id_from_ed25519(ed25519_pub: &[u8; KEY_LEN]) -> String {
    let digest = primitives::blake3_hash(ed25519_pub);
    let mut s = String::with_capacity(DEVICE_ID_LEN * 2);
    for b in &digest[..DEVICE_ID_LEN] {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    s
}

/// Saturating length conversion for wire framing.
fn u32_len(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn device_record_signs_and_verifies() {
        let kp = DeviceKeypairs::generate().unwrap();
        let signed = kp.sign_record(1_700_000_000_000);
        assert!(signed.verify().is_ok());
        assert_eq!(signed.record.device_id, kp.device_id());
    }

    #[test]
    fn tampered_record_fails_verification() {
        let kp = DeviceKeypairs::generate().unwrap();
        let mut signed = kp.sign_record(1);
        signed.record.created_at = 2;
        assert!(signed.verify().is_err());
    }

    #[test]
    fn forged_device_id_is_rejected() {
        let kp = DeviceKeypairs::generate().unwrap();
        let mut signed = kp.sign_record(1);
        signed.record.device_id = "deadbeef".to_owned();
        assert!(matches!(signed.verify(), Err(CryptoError::Malformed(_))));
    }

    #[test]
    fn reconstructed_keypairs_have_same_identity() {
        // from_secrets must yield a stable device_id and a verifiable record for
        // given secret scalars, so a device can reload its keys from a keychain.
        let x_seed = [7u8; KEY_LEN];
        let ed_seed = [9u8; KEY_LEN];
        let a = DeviceKeypairs::from_secrets(x_seed, ed_seed);
        let b = DeviceKeypairs::from_secrets(x_seed, ed_seed);
        assert_eq!(a.device_id(), b.device_id());
        assert_eq!(a.x25519_public(), b.x25519_public());
        assert_eq!(a.ed25519_verifying(), b.ed25519_verifying());

        let signed = a.sign_record(1_700_000_000_000);
        assert!(signed.verify().is_ok());
    }
}
