// SPDX-License-Identifier: Apache-2.0

//! Device-to-device onboarding: Short Authentication String (SAS) verification
//! and sealed enrollment bundles (ADR-024).
//!
//! A new device is authorized by an existing trusted device. The two verify a
//! **SAS** out-of-band (QR / short code) to defeat a man-in-the-middle, then the
//! trusted device seals the account secret to the new device's X25519 key. The
//! server only relays the opaque sealed bundle.
//!
//! The bundle carries the Account Root Key, the opaque `account_id`, and the
//! current `key_epoch`; the new device derives every epoch key and the
//! `account_stream_key` from the ARK once it opens the bundle.

use crate::error::{CryptoError, Result};
use crate::hierarchy::{ACCOUNT_ID_LEN, AccountId, AccountRootKey};
use crate::primitives::{self, KEY_LEN};

/// Domain tag prefixed to the SAS commitment input.
const SAS_TAG: &[u8] = b"pergamon/v1/enrollment-sas";
/// Domain tag prefixed to the enrollment bundle plaintext.
const BUNDLE_TAG: &[u8] = b"pergamon/v1/enrollment-bundle";
/// Domain tag prefixed to the enrollment sealed-box AAD.
const ENROLL_AAD_TAG: &[u8] = b"pergamon/v1/enrollment-aad";

/// Byte length of the sealed bundle plaintext:
/// tag ‖ ARK(32) ‖ `account_id`(16) ‖ epoch(4).
const BUNDLE_PLAINTEXT_LEN: usize = BUNDLE_TAG.len() + KEY_LEN + ACCOUNT_ID_LEN + 4;

/// One device's enrollment public keys, as shown to the other for SAS
/// comparison.
#[derive(Debug, Clone, Copy)]
pub struct EnrollmentPeer {
    /// The device's X25519 public key.
    pub x25519_pub: [u8; KEY_LEN],
    /// The device's Ed25519 verifying key.
    pub ed25519_pub: [u8; KEY_LEN],
}

impl EnrollmentPeer {
    /// Concatenate the peer's two public keys in fixed order.
    fn concat(&self) -> [u8; KEY_LEN * 2] {
        let mut out = [0u8; KEY_LEN * 2];
        out[..KEY_LEN].copy_from_slice(&self.x25519_pub);
        out[KEY_LEN..].copy_from_slice(&self.ed25519_pub);
        out
    }
}

/// A Short Authentication String: a BLAKE3 commitment over **both** devices'
/// enrollment public keys.
///
/// Both devices compute the same value regardless of role (the two peers are
/// sorted into a canonical order first), so if a MITM swapped either key the
/// commitments — and thus the displayed codes — differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sas {
    commitment: [u8; 32],
}

impl Sas {
    /// Compute the SAS from the two peers' enrollment public keys.
    ///
    /// The inputs are ordered canonically so both sides agree; the caller may
    /// pass `local` and `remote` in either role.
    #[must_use]
    pub fn compute(local: &EnrollmentPeer, remote: &EnrollmentPeer) -> Self {
        let a = local.concat();
        let b = remote.concat();
        // Canonical order: smaller byte string first.
        let (first, second) = if a <= b { (a, b) } else { (b, a) };
        let mut input = Vec::with_capacity(SAS_TAG.len() + a.len() + b.len());
        input.extend_from_slice(SAS_TAG);
        input.extend_from_slice(&first);
        input.extend_from_slice(&second);
        Self {
            commitment: primitives::blake3_hash(&input),
        }
    }

    /// The full 32-byte commitment (e.g. to render as a QR code).
    #[must_use]
    pub const fn commitment(&self) -> &[u8; 32] {
        &self.commitment
    }

    /// A human-comparable decimal code: twelve digits grouped as `NNNN NNNN
    /// NNNN`, derived from the commitment. Both devices show the same code iff
    /// the underlying public keys match.
    #[must_use]
    pub fn digits(&self) -> String {
        let mut n: u64 = 0;
        for b in &self.commitment[..8] {
            n = (n << 8) | u64::from(*b);
        }
        let code = n % 1_000_000_000_000; // 12 digits
        let s = format!("{code:012}");
        format!("{} {} {}", &s[0..4], &s[4..8], &s[8..12])
    }

    /// Constant-time-ish comparison of two SAS commitments (both values are
    /// public, so this is defense-in-depth rather than a strict requirement).
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        let mut diff = 0u8;
        for (x, y) in self.commitment.iter().zip(other.commitment.iter()) {
            diff |= x ^ y;
        }
        diff == 0
    }
}

/// The secret payload sealed to a new device during enrollment.
///
/// Deliberately does not derive `Debug`: it wraps the Account Root Key.
#[derive(Clone)]
pub struct EnrollmentBundle {
    /// The account root key — the new device derives everything from it.
    pub ark: AccountRootKey,
    /// The opaque account handle.
    pub account_id: AccountId,
    /// The account's current key epoch at the time of enrollment.
    pub key_epoch: u32,
}

/// Seal an enrollment bundle to a new device's X25519 public key.
///
/// The trusted device calls this after the SAS has been verified out-of-band.
/// The sealed box is opaque to the server, which merely relays it. The AAD binds
/// the recipient `device_id` so the server cannot re-target the bundle to a
/// different device.
///
/// # Errors
/// Returns a crypto error if sealing fails.
pub fn seal_enrollment_bundle(
    recipient_x25519_pub: &[u8; KEY_LEN],
    recipient_device_id: &str,
    ark: &AccountRootKey,
    account_id: &AccountId,
    key_epoch: u32,
) -> Result<Vec<u8>> {
    let mut plaintext = Vec::with_capacity(BUNDLE_PLAINTEXT_LEN);
    plaintext.extend_from_slice(BUNDLE_TAG);
    plaintext.extend_from_slice(ark.expose_bytes());
    plaintext.extend_from_slice(account_id.as_bytes());
    plaintext.extend_from_slice(&key_epoch.to_be_bytes());

    let aad = enrollment_aad(recipient_device_id);
    primitives::seal_to(recipient_x25519_pub, &aad, &plaintext)
}

/// Open an enrollment bundle sealed to this device.
///
/// # Errors
/// Returns [`CryptoError::Decryption`] if the bundle was not sealed to this
/// device (or was tampered with), or [`CryptoError::Malformed`] if the decrypted
/// plaintext is not a well-formed bundle.
pub fn open_enrollment_bundle(
    recipient_x25519_secret: &[u8; KEY_LEN],
    recipient_device_id: &str,
    sealed: &[u8],
) -> Result<EnrollmentBundle> {
    let aad = enrollment_aad(recipient_device_id);
    let plaintext = primitives::open_sealed(recipient_x25519_secret, &aad, sealed)?;
    if plaintext.len() != BUNDLE_PLAINTEXT_LEN || !plaintext.starts_with(BUNDLE_TAG) {
        return Err(CryptoError::Malformed("malformed enrollment bundle"));
    }
    let mut off = BUNDLE_TAG.len();

    let mut ark_bytes = [0u8; KEY_LEN];
    ark_bytes.copy_from_slice(&plaintext[off..off + KEY_LEN]);
    off += KEY_LEN;

    let mut id_bytes = [0u8; ACCOUNT_ID_LEN];
    id_bytes.copy_from_slice(&plaintext[off..off + ACCOUNT_ID_LEN]);
    off += ACCOUNT_ID_LEN;

    let mut epoch_bytes = [0u8; 4];
    epoch_bytes.copy_from_slice(&plaintext[off..off + 4]);

    Ok(EnrollmentBundle {
        ark: AccountRootKey::from_bytes(ark_bytes),
        account_id: AccountId::from_bytes(id_bytes),
        key_epoch: u32::from_be_bytes(epoch_bytes),
    })
}

/// Build the enrollment sealed-box AAD binding the recipient device handle.
fn enrollment_aad(recipient_device_id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(ENROLL_AAD_TAG.len() + recipient_device_id.len());
    aad.extend_from_slice(ENROLL_AAD_TAG);
    aad.extend_from_slice(recipient_device_id.as_bytes());
    aad
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::device::DeviceKeypairs;

    #[test]
    fn sas_matches_for_both_devices_and_detects_swap() {
        let e = DeviceKeypairs::generate().unwrap();
        let n = DeviceKeypairs::generate().unwrap();
        let peer_e = EnrollmentPeer {
            x25519_pub: *e.x25519_public(),
            ed25519_pub: *e.ed25519_verifying(),
        };
        let peer_n = EnrollmentPeer {
            x25519_pub: *n.x25519_public(),
            ed25519_pub: *n.ed25519_verifying(),
        };
        // Each device computes SAS(self, other); results agree.
        let sas_on_e = Sas::compute(&peer_e, &peer_n);
        let sas_on_n = Sas::compute(&peer_n, &peer_e);
        assert!(sas_on_e.matches(&sas_on_n));
        assert_eq!(sas_on_e.digits(), sas_on_n.digits());

        // A MITM that swapped the new device's key yields a different SAS.
        let mitm = DeviceKeypairs::generate().unwrap();
        let peer_mitm = EnrollmentPeer {
            x25519_pub: *mitm.x25519_public(),
            ed25519_pub: *mitm.ed25519_verifying(),
        };
        let sas_mitm = Sas::compute(&peer_e, &peer_mitm);
        assert!(!sas_on_e.matches(&sas_mitm));
    }

    #[test]
    fn sas_digits_shape() {
        let e = DeviceKeypairs::generate().unwrap();
        let n = DeviceKeypairs::generate().unwrap();
        let sas = Sas::compute(
            &EnrollmentPeer {
                x25519_pub: *e.x25519_public(),
                ed25519_pub: *e.ed25519_verifying(),
            },
            &EnrollmentPeer {
                x25519_pub: *n.x25519_public(),
                ed25519_pub: *n.ed25519_verifying(),
            },
        );
        let d = sas.digits();
        assert_eq!(d.len(), 14); // "NNNN NNNN NNNN"
        assert_eq!(d.chars().filter(char::is_ascii_digit).count(), 12);
    }

    #[test]
    fn enrollment_bundle_roundtrip() {
        let new_device = DeviceKeypairs::generate().unwrap();
        let ark = AccountRootKey::from_bytes([21u8; 32]);
        let account_id = AccountId::from_bytes([7u8; ACCOUNT_ID_LEN]);

        let sealed = seal_enrollment_bundle(
            new_device.x25519_public(),
            new_device.device_id(),
            &ark,
            &account_id,
            3,
        )
        .unwrap();

        let opened =
            open_enrollment_bundle(new_device.x25519_secret(), new_device.device_id(), &sealed)
                .unwrap();

        assert_eq!(opened.ark.expose_bytes(), ark.expose_bytes());
        assert_eq!(opened.account_id, account_id);
        assert_eq!(opened.key_epoch, 3);
    }

    #[test]
    fn bundle_sealed_to_other_device_cannot_open() {
        let target = DeviceKeypairs::generate().unwrap();
        let attacker = DeviceKeypairs::generate().unwrap();
        let ark = AccountRootKey::from_bytes([21u8; 32]);
        let account_id = AccountId::from_bytes([7u8; ACCOUNT_ID_LEN]);

        let sealed = seal_enrollment_bundle(
            target.x25519_public(),
            target.device_id(),
            &ark,
            &account_id,
            0,
        )
        .unwrap();

        assert!(
            open_enrollment_bundle(attacker.x25519_secret(), attacker.device_id(), &sealed)
                .is_err()
        );
    }

    #[test]
    fn bundle_retargeted_device_id_fails_aad() {
        let target = DeviceKeypairs::generate().unwrap();
        let ark = AccountRootKey::from_bytes([21u8; 32]);
        let account_id = AccountId::from_bytes([7u8; ACCOUNT_ID_LEN]);
        let sealed = seal_enrollment_bundle(
            target.x25519_public(),
            target.device_id(),
            &ark,
            &account_id,
            0,
        )
        .unwrap();
        // Same key, but AAD claims a different device_id => open fails.
        assert!(
            open_enrollment_bundle(target.x25519_secret(), "different-device-id", &sealed).is_err()
        );
    }
}
