// SPDX-License-Identifier: Apache-2.0

//! Key-epoch rotation on device revocation (ADR-024).
//!
//! Revoking a device advances the account's `key_epoch`: a remaining trusted
//! device derives the next account content key `ACK_{e+1}` and **re-wraps** it to
//! every remaining device with an X25519 sealed box — but never to the revoked
//! device. New content is then encrypted under `ACK_{e+1}`.
//!
//! The wrapped bundle's AEAD associated data binds the recipient `device_id`,
//! the `account_id`, and the target epoch, so the blind server cannot re-target a
//! re-wrap to another device or replay it at a different epoch.
//!
//! **Secrecy boundary (honest).** Rotation protects *future* content only. A
//! revoked device keeps whatever epoch keys and plaintext it already held; true
//! forward secrecy for old content would require a full library re-key, which is
//! out of scope (ADR-024).

use crate::error::{CryptoError, Result};
use crate::hierarchy::{AccountContentKey, AccountId, AccountRootKey};
use crate::primitives::{self, KEY_LEN};

/// Domain tag prefixed to the re-wrap bundle plaintext.
const REWRAP_TAG: &[u8] = b"pergamon/v1/rewrap-bundle";
/// Domain tag prefixed to the re-wrap sealed-box AAD.
const REWRAP_AAD_TAG: &[u8] = b"pergamon/v1/rewrap-aad";

/// Plaintext length of a re-wrap bundle: tag ‖ epoch(4) ‖ ACK(32).
const REWRAP_PLAINTEXT_LEN: usize = REWRAP_TAG.len() + 4 + KEY_LEN;

/// A recipient device an epoch key is re-wrapped to.
#[derive(Debug, Clone, Copy)]
pub struct RewrapRecipient<'a> {
    /// The recipient's opaque device handle (bound into the AAD).
    pub device_id: &'a str,
    /// The recipient's X25519 public key (the sealed-box target).
    pub x25519_pub: &'a [u8; KEY_LEN],
}

/// A per-device sealed re-wrap of the new epoch key, ready to relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewrappedKey {
    /// The recipient device the sealed box is addressed to.
    pub device_id: String,
    /// The epoch this bundle carries.
    pub key_epoch: u32,
    /// Opaque sealed box carrying `ACK_{key_epoch}`.
    pub sealed: Vec<u8>,
}

/// Advance to `new_epoch` and re-wrap the new account content key to each
/// remaining device.
///
/// `new_epoch` is normally the current epoch plus one; the revoked device is
/// simply omitted from `recipients`. Returns the freshly derived
/// [`AccountContentKey`] (for the caller to encrypt new content with) alongside
/// the per-device sealed bundles.
///
/// # Errors
/// Returns a crypto error if key derivation or sealing fails.
pub fn rotate_and_rewrap(
    ark: &AccountRootKey,
    account_id: &AccountId,
    new_epoch: u32,
    recipients: &[RewrapRecipient<'_>],
) -> Result<(AccountContentKey, Vec<RewrappedKey>)> {
    let ack = ark.content_key(new_epoch)?;
    let mut plaintext = Vec::with_capacity(REWRAP_PLAINTEXT_LEN);
    plaintext.extend_from_slice(REWRAP_TAG);
    plaintext.extend_from_slice(&new_epoch.to_be_bytes());
    plaintext.extend_from_slice(ack.expose_bytes());

    let mut wrapped = Vec::with_capacity(recipients.len());
    for r in recipients {
        let aad = rewrap_aad(account_id, r.device_id, new_epoch);
        let sealed = primitives::seal_to(r.x25519_pub, &aad, &plaintext)?;
        wrapped.push(RewrappedKey {
            device_id: r.device_id.to_owned(),
            key_epoch: new_epoch,
            sealed,
        });
    }
    Ok((ack, wrapped))
}

/// Open a re-wrap bundle addressed to this device and recover the new epoch key.
///
/// # Errors
/// [`CryptoError::Decryption`] if the bundle was not sealed to this device at
/// this epoch (wrong recipient or AAD mismatch); [`CryptoError::Malformed`] if
/// the plaintext is not a well-formed re-wrap bundle or its epoch disagrees with
/// `expected_epoch`.
pub fn open_rewrapped(
    recipient_x25519_secret: &[u8; KEY_LEN],
    recipient_device_id: &str,
    account_id: &AccountId,
    expected_epoch: u32,
    sealed: &[u8],
) -> Result<AccountContentKey> {
    let aad = rewrap_aad(account_id, recipient_device_id, expected_epoch);
    let plaintext = primitives::open_sealed(recipient_x25519_secret, &aad, sealed)?;
    if plaintext.len() != REWRAP_PLAINTEXT_LEN || !plaintext.starts_with(REWRAP_TAG) {
        return Err(CryptoError::Malformed("malformed re-wrap bundle"));
    }
    let mut off = REWRAP_TAG.len();

    let mut epoch_bytes = [0u8; 4];
    epoch_bytes.copy_from_slice(&plaintext[off..off + 4]);
    off += 4;
    let epoch = u32::from_be_bytes(epoch_bytes);
    if epoch != expected_epoch {
        return Err(CryptoError::Malformed("re-wrap epoch mismatch"));
    }

    let mut ack_bytes = [0u8; KEY_LEN];
    ack_bytes.copy_from_slice(&plaintext[off..off + KEY_LEN]);
    Ok(AccountContentKey::from_bytes(epoch, ack_bytes))
}

/// Build the re-wrap sealed-box AAD binding account, recipient device, and
/// epoch.
fn rewrap_aad(account_id: &AccountId, device_id: &str, epoch: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(REWRAP_AAD_TAG.len() + KEY_LEN + device_id.len() + 8);
    aad.extend_from_slice(REWRAP_AAD_TAG);
    aad.extend_from_slice(account_id.as_bytes());
    let len = u32::try_from(device_id.len()).unwrap_or(u32::MAX);
    aad.extend_from_slice(&len.to_be_bytes());
    aad.extend_from_slice(device_id.as_bytes());
    aad.extend_from_slice(&epoch.to_be_bytes());
    aad
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::device::DeviceKeypairs;
    use crate::hierarchy::ACCOUNT_ID_LEN;

    #[test]
    fn rewrap_roundtrip_to_remaining_devices() {
        let ark = AccountRootKey::from_bytes([11u8; KEY_LEN]);
        let account_id = AccountId::from_bytes([2u8; ACCOUNT_ID_LEN]);
        let dev_a = DeviceKeypairs::generate().unwrap();
        let dev_b = DeviceKeypairs::generate().unwrap();

        let recipients = [
            RewrapRecipient {
                device_id: dev_a.device_id(),
                x25519_pub: dev_a.x25519_public(),
            },
            RewrapRecipient {
                device_id: dev_b.device_id(),
                x25519_pub: dev_b.x25519_public(),
            },
        ];
        let (ack_new, wraps) = rotate_and_rewrap(&ark, &account_id, 1, &recipients).unwrap();
        assert_eq!(wraps.len(), 2);

        for (dev, wrap) in [(&dev_a, &wraps[0]), (&dev_b, &wraps[1])] {
            let opened = open_rewrapped(
                dev.x25519_secret(),
                dev.device_id(),
                &account_id,
                1,
                &wrap.sealed,
            )
            .unwrap();
            assert_eq!(opened.epoch(), 1);
            assert_eq!(opened.expose_bytes(), ack_new.expose_bytes());
        }
    }

    #[test]
    fn rewrapped_key_matches_direct_derivation() {
        let ark = AccountRootKey::from_bytes([11u8; KEY_LEN]);
        let account_id = AccountId::from_bytes([2u8; ACCOUNT_ID_LEN]);
        let dev = DeviceKeypairs::generate().unwrap();
        let recipients = [RewrapRecipient {
            device_id: dev.device_id(),
            x25519_pub: dev.x25519_public(),
        }];
        let (_, wraps) = rotate_and_rewrap(&ark, &account_id, 4, &recipients).unwrap();
        let opened = open_rewrapped(
            dev.x25519_secret(),
            dev.device_id(),
            &account_id,
            4,
            &wraps[0].sealed,
        )
        .unwrap();
        // The unwrapped key equals what a holder of the ARK derives directly.
        let direct = ark.content_key(4).unwrap();
        assert_eq!(opened.expose_bytes(), direct.expose_bytes());
    }

    #[test]
    fn revoked_device_is_omitted() {
        let ark = AccountRootKey::from_bytes([11u8; KEY_LEN]);
        let account_id = AccountId::from_bytes([2u8; ACCOUNT_ID_LEN]);
        let keep = DeviceKeypairs::generate().unwrap();
        let revoked = DeviceKeypairs::generate().unwrap();

        // Only the retained device is a recipient.
        let recipients = [RewrapRecipient {
            device_id: keep.device_id(),
            x25519_pub: keep.x25519_public(),
        }];
        let (_, wraps) = rotate_and_rewrap(&ark, &account_id, 1, &recipients).unwrap();
        assert_eq!(wraps.len(), 1);
        assert_eq!(wraps[0].device_id, keep.device_id());

        // The revoked device cannot open the retained device's bundle.
        assert!(
            open_rewrapped(
                revoked.x25519_secret(),
                revoked.device_id(),
                &account_id,
                1,
                &wraps[0].sealed,
            )
            .is_err()
        );
    }

    #[test]
    fn wrong_epoch_aad_fails() {
        let ark = AccountRootKey::from_bytes([11u8; KEY_LEN]);
        let account_id = AccountId::from_bytes([2u8; ACCOUNT_ID_LEN]);
        let dev = DeviceKeypairs::generate().unwrap();
        let recipients = [RewrapRecipient {
            device_id: dev.device_id(),
            x25519_pub: dev.x25519_public(),
        }];
        let (_, wraps) = rotate_and_rewrap(&ark, &account_id, 1, &recipients).unwrap();
        // Claiming epoch 2 changes the AAD => open fails.
        assert!(
            open_rewrapped(
                dev.x25519_secret(),
                dev.device_id(),
                &account_id,
                2,
                &wraps[0].sealed
            )
            .is_err()
        );
    }
}
