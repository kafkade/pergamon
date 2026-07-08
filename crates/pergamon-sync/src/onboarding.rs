// SPDX-License-Identifier: Apache-2.0

//! Device onboarding, revocation, and recovery orchestration (ADR-024, #128).
//!
//! This module wires the `pergamon-crypto` primitives to a [`RelayTransport`] to
//! implement the client-side flows the CLI drives:
//!
//! - [`bootstrap`] — the first device publishes its signed record and a
//!   self-trust attestation, founding the account roster.
//! - [`enroll_publish`] — a new device publishes its record so an existing
//!   device can find and approve it.
//! - [`sas_against`] — compute the out-of-band Short Authentication String
//!   between the local device and a named roster peer.
//! - [`approve`] — an existing trusted device seals the account secret to a new
//!   device and vouches for it.
//! - [`accept`] — the new device opens the sealed bundle and confirms it was
//!   vouched for, yielding the Account Root Key.
//! - [`revoke`] — a trusted device rotates the key epoch, re-wraps the new epoch
//!   key to every remaining device, and publishes a revocation attestation.
//! - [`recovery_publish`] / [`recover_ark`] — the optional passphrase-wrapped
//!   recovery path.
//!
//! Every function takes the wall-clock `now_millis` from the caller (the crypto
//! layer is clock-free per ADR-001) and treats all relayed bytes as opaque; the
//! server never learns anything it relays.

use pergamon_crypto::device::{DeviceKeypairs, DeviceRecord, SignedDeviceRecord};
use pergamon_crypto::enrollment::{EnrollmentBundle, EnrollmentPeer, Sas};
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_crypto::recovery::RecoveryBlob;
use pergamon_crypto::rotation::RewrapRecipient;
use pergamon_crypto::rotation::rotate_and_rewrap;
use pergamon_crypto::{
    AttestationKind, SignedAttestation, attest_revoke, attest_trust, enable_recovery,
    open_enrollment_bundle, recover, seal_enrollment_bundle,
};

use crate::error::{Result, SyncError};
use crate::relay::RelayTransport;

/// Map a crypto error into a sync error with context.
fn crypto_err(what: &str, e: &pergamon_crypto::CryptoError) -> SyncError {
    SyncError::Protocol(format!("{what}: {e}"))
}

/// Build the local device's enrollment peer view (its two public keys).
const fn local_peer(keys: &DeviceKeypairs) -> EnrollmentPeer {
    EnrollmentPeer {
        x25519_pub: *keys.x25519_public(),
        ed25519_pub: *keys.ed25519_verifying(),
    }
}

/// Build a remote peer view from a verified device record.
const fn record_peer(record: &DeviceRecord) -> EnrollmentPeer {
    EnrollmentPeer {
        x25519_pub: record.x25519_pub,
        ed25519_pub: record.ed25519_pub,
    }
}

/// Publish this device's self-signed record to the account roster.
///
/// # Errors
/// Returns a [`SyncError`] if the transport fails.
pub fn enroll_publish<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    keys: &DeviceKeypairs,
    now_millis: i64,
) -> Result<()> {
    let signed = keys.sign_record(now_millis);
    relay.device_put(&account_id.to_hex(), keys.device_id(), &signed.to_bytes())
}

/// Bootstrap the first device on a new account: publish its device record and a
/// self-trust attestation rooting the account's web of trust.
///
/// # Errors
/// Returns a [`SyncError`] if the transport fails.
pub fn bootstrap<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    keys: &DeviceKeypairs,
    key_epoch: u32,
    now_millis: i64,
) -> Result<()> {
    let signed = keys.sign_record(now_millis);
    let account_hex = account_id.to_hex();
    relay.device_put(&account_hex, keys.device_id(), &signed.to_bytes())?;
    // A self-trust attestation roots the trust chain: this device vouches for
    // itself at the founding epoch.
    let attestation = attest_trust(keys, &signed.record, key_epoch, now_millis);
    relay.attestation_append(&account_hex, &attestation.to_bytes())?;
    Ok(())
}

/// Fetch and verify one device's record from the roster.
///
/// # Errors
/// [`SyncError::NotFound`] if the device is absent; [`SyncError::Protocol`] if
/// the record is malformed or its self-signature does not verify.
pub fn fetch_device_record<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    device_id: &str,
) -> Result<SignedDeviceRecord> {
    let bytes = relay
        .device_get(&account_id.to_hex(), device_id)?
        .ok_or_else(|| SyncError::NotFound(format!("device record for {device_id}")))?;
    let signed = SignedDeviceRecord::from_bytes(&bytes)
        .map_err(|e| crypto_err("parsing device record", &e))?;
    signed
        .verify()
        .map_err(|e| crypto_err("verifying device record", &e))?;
    Ok(signed)
}

/// List and verify the account's full device roster.
///
/// Records that fail to parse or verify are skipped rather than aborting the
/// whole listing, so one bad record cannot deny the roster.
///
/// # Errors
/// Returns a [`SyncError`] if the transport fails.
pub fn roster<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
) -> Result<Vec<SignedDeviceRecord>> {
    let raw = relay.devices_list(&account_id.to_hex())?;
    let mut out = Vec::with_capacity(raw.len());
    for d in raw {
        if let Ok(signed) = SignedDeviceRecord::from_bytes(&d.record)
            && signed.verify().is_ok()
            && signed.record.device_id == d.device_id
        {
            out.push(signed);
        }
    }
    Ok(out)
}

/// Compute the Short Authentication String between the local device and the
/// named roster peer, for out-of-band human comparison.
///
/// # Errors
/// [`SyncError::NotFound`] if the peer is absent; a [`SyncError`] on transport
/// or verification failure.
pub fn sas_against<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    local: &DeviceKeypairs,
    peer_device_id: &str,
) -> Result<Sas> {
    let peer = fetch_device_record(relay, account_id, peer_device_id)?;
    Ok(Sas::compute(&local_peer(local), &record_peer(&peer.record)))
}

/// An existing trusted device approves a new device: it seals the account secret
/// to the new device and publishes a trust attestation vouching for it.
///
/// The caller is expected to have already verified the SAS out-of-band (see
/// [`sas_against`]); this function performs the cryptographic authorization.
/// Returns the computed SAS so the caller can display/re-check it.
///
/// # Errors
/// [`SyncError::NotFound`] if the subject is absent; a [`SyncError`] on
/// transport, sealing, or verification failure.
pub fn approve<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    approver: &DeviceKeypairs,
    ark: &AccountRootKey,
    key_epoch: u32,
    subject_device_id: &str,
    now_millis: i64,
) -> Result<Sas> {
    let account_hex = account_id.to_hex();
    let subject = fetch_device_record(relay, account_id, subject_device_id)?;
    let sas = Sas::compute(&local_peer(approver), &record_peer(&subject.record));

    // Seal the enrollment bundle (ARK, account_id, epoch) to the new device's
    // X25519 key, AAD-bound to its handle so the server cannot re-target it.
    let sealed = seal_enrollment_bundle(
        &subject.record.x25519_pub,
        &subject.record.device_id,
        ark,
        account_id,
        key_epoch,
    )
    .map_err(|e| crypto_err("sealing enrollment bundle", &e))?;
    relay.wrap_put(&account_hex, &subject.record.device_id, &sealed)?;

    // Vouch for the new device so the roster records the grant of trust.
    let attestation = attest_trust(approver, &subject.record, key_epoch, now_millis);
    relay.attestation_append(&account_hex, &attestation.to_bytes())?;
    Ok(sas)
}

/// The outcome of a successful [`accept`]: the recovered account secret and the
/// device that vouched for this one.
pub struct Accepted {
    /// The opened enrollment bundle carrying the ARK, account id, and epoch.
    pub bundle: EnrollmentBundle,
    /// The handle of the trusted device that approved this enrollment, if a
    /// matching trust attestation was found on the roster.
    pub approver_device_id: Option<String>,
}

/// A new device opens the sealed bundle addressed to it and confirms it was
/// vouched for, yielding the Account Root Key and epoch to store locally.
///
/// Opening the sealed box already proves the bundle was produced by a holder of
/// the ARK and sealed specifically to this device (AAD-bound); this additionally
/// looks for a matching trust attestation from a current roster device as
/// roster bookkeeping.
///
/// # Errors
/// [`SyncError::NotFound`] if no bundle has been sealed to this device yet;
/// a [`SyncError`] on transport failure or if no bundle opens.
pub fn accept<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    keys: &DeviceKeypairs,
) -> Result<Accepted> {
    let account_hex = account_id.to_hex();
    let wraps = relay.wraps_list(&account_hex, keys.device_id(), 0)?;
    if wraps.is_empty() {
        return Err(SyncError::NotFound(format!(
            "no enrollment bundle for device {} yet; ask a trusted device to approve it",
            keys.device_id()
        )));
    }
    // Try each pending wrap; accept the first that opens for us.
    let mut bundle = None;
    for w in &wraps {
        if let Ok(opened) =
            open_enrollment_bundle(keys.x25519_secret(), keys.device_id(), &w.bundle)
        {
            bundle = Some(opened);
            break;
        }
    }
    let bundle = bundle.ok_or_else(|| {
        SyncError::Protocol("no relayed bundle could be opened by this device".to_owned())
    })?;

    // Look for a verifiable trust attestation naming this device as subject.
    let approver_device_id = find_truster(relay, &account_hex, keys.device_id())?;
    Ok(Accepted {
        bundle,
        approver_device_id,
    })
}

/// Scan the account's attestation history for a verifiable `Trust` attestation
/// whose subject is `subject_device_id`, returning the signer's handle.
fn find_truster<R: RelayTransport>(
    relay: &R,
    account_hex: &str,
    subject_device_id: &str,
) -> Result<Option<String>> {
    let attestations = relay.attestations_list(account_hex, 0)?;
    for a in attestations {
        let Ok(signed) = SignedAttestation::from_bytes(&a.attestation) else {
            continue;
        };
        if signed.attestation.kind == AttestationKind::Trust
            && signed.attestation.subject_device_id == subject_device_id
            && signed.verify().is_ok()
        {
            return Ok(Some(signed.attestation.signer_device_id));
        }
    }
    Ok(None)
}

/// The result of a revocation: the new epoch and the devices re-wrapped to.
pub struct Revocation {
    /// The new key epoch that now encrypts fresh content.
    pub new_epoch: u32,
    /// The remaining devices the new epoch key was re-wrapped to.
    pub rewrapped_devices: Vec<String>,
}

/// Revoke a device: advance to `current_epoch + 1`, re-wrap the new account
/// content key to every remaining device, and publish a revocation attestation.
///
/// **Secrecy boundary (ADR-024, stated honestly):** every enrolled device holds
/// the Account Root Key, so a revoked device can still derive future epoch keys
/// on its own. Rotation here is roster hygiene plus an epoch advance that lets
/// remaining devices agree on the current epoch; a full forward-secrecy re-key
/// of already-uploaded content is explicitly out of scope.
///
/// # Errors
/// [`SyncError::NotFound`] if the revoked device is not on the roster; a
/// [`SyncError`] on transport or crypto failure.
pub fn revoke<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    approver: &DeviceKeypairs,
    ark: &AccountRootKey,
    current_epoch: u32,
    revoked_device_id: &str,
    now_millis: i64,
) -> Result<Revocation> {
    let account_hex = account_id.to_hex();
    let all = roster(relay, account_id)?;
    let revoked = all
        .iter()
        .find(|r| r.record.device_id == revoked_device_id)
        .ok_or_else(|| {
            SyncError::NotFound(format!("device {revoked_device_id} is not on the roster"))
        })?
        .clone();

    let new_epoch = current_epoch.saturating_add(1);
    let remaining: Vec<&SignedDeviceRecord> = all
        .iter()
        .filter(|r| r.record.device_id != revoked_device_id)
        .collect();
    let recipients: Vec<RewrapRecipient<'_>> = remaining
        .iter()
        .map(|r| RewrapRecipient {
            device_id: &r.record.device_id,
            x25519_pub: &r.record.x25519_pub,
        })
        .collect();

    let (_ack, wraps) = rotate_and_rewrap(ark, account_id, new_epoch, &recipients)
        .map_err(|e| crypto_err("re-wrapping epoch key", &e))?;
    for w in &wraps {
        relay.wrap_put(&account_hex, &w.device_id, &w.sealed)?;
    }

    let attestation = attest_revoke(approver, &revoked.record, new_epoch, now_millis);
    relay.attestation_append(&account_hex, &attestation.to_bytes())?;

    Ok(Revocation {
        new_epoch,
        rewrapped_devices: wraps.into_iter().map(|w| w.device_id).collect(),
    })
}

/// Enable recovery: wrap the ARK under a passphrase/recovery-code-derived key and
/// upload the opaque blob so a future device with no trusted peer can restore.
///
/// # Errors
/// Returns a [`SyncError`] on crypto or transport failure.
pub fn recovery_publish<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    ark: &AccountRootKey,
    secret: &[u8],
) -> Result<()> {
    let blob = enable_recovery(ark, account_id, secret)
        .map_err(|e| crypto_err("building recovery blob", &e))?;
    relay.recovery_put(&account_id.to_hex(), &blob.to_bytes())
}

/// Recover the Account Root Key from the server's recovery blob and the account
/// secret, for a fresh device with no trusted peer.
///
/// # Errors
/// [`SyncError::NotFound`] if recovery is not enabled for the account;
/// [`SyncError::Protocol`] if the secret is wrong or the blob is malformed.
pub fn recover_ark<R: RelayTransport>(
    relay: &R,
    account_id: &AccountId,
    secret: &[u8],
) -> Result<AccountRootKey> {
    let bytes = relay.recovery_get(&account_id.to_hex())?.ok_or_else(|| {
        SyncError::NotFound("recovery is not enabled for this account".to_owned())
    })?;
    let blob =
        RecoveryBlob::from_bytes(&bytes).map_err(|e| crypto_err("parsing recovery blob", &e))?;
    recover(&blob, account_id, secret).map_err(|e| crypto_err("recovering account key", &e))
}

/// Determine the account's current key epoch by scanning the attestation
/// history for the greatest epoch any verifiable attestation names.
///
/// A fresh device recovering with no trusted peer uses this to bind its local
/// `key_epoch` to the account's current epoch, so new content it writes is
/// encrypted under the live epoch rather than a stale one. Returns `0` when the
/// account has no attestations yet.
///
/// # Errors
/// Returns a [`SyncError`] if the transport fails.
pub fn current_epoch<R: RelayTransport>(relay: &R, account_id: &AccountId) -> Result<u32> {
    let attestations = relay.attestations_list(&account_id.to_hex(), 0)?;
    let mut epoch = 0u32;
    for a in attestations {
        if let Ok(signed) = SignedAttestation::from_bytes(&a.attestation)
            && signed.verify().is_ok()
        {
            epoch = epoch.max(signed.attestation.key_epoch);
        }
    }
    Ok(epoch)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::relay::MemoryRelay;

    const NOW: i64 = 1_700_000_000_000;

    fn account() -> (AccountRootKey, AccountId) {
        (
            AccountRootKey::from_bytes([5u8; 32]),
            AccountId::from_bytes([9u8; 16]),
        )
    }

    #[test]
    fn full_enrollment_transfers_ark_to_new_device() {
        let relay = MemoryRelay::new();
        let (ark, id) = account();
        let dev_a = DeviceKeypairs::generate().unwrap();
        let dev_b = DeviceKeypairs::generate().unwrap();

        // A bootstraps the account.
        bootstrap(&relay, &id, &dev_a, 0, NOW).unwrap();
        // B publishes its record and can be seen on the roster.
        enroll_publish(&relay, &id, &dev_b, NOW).unwrap();
        assert_eq!(roster(&relay, &id).unwrap().len(), 2);

        // Both sides compute the same SAS.
        let sas_a = sas_against(&relay, &id, &dev_a, dev_b.device_id()).unwrap();
        let sas_b = sas_against(&relay, &id, &dev_b, dev_a.device_id()).unwrap();
        assert!(sas_a.matches(&sas_b));

        // A approves B; B accepts and recovers the ARK.
        approve(&relay, &id, &dev_a, &ark, 0, dev_b.device_id(), NOW).unwrap();
        let accepted = accept(&relay, &id, &dev_b).unwrap();
        assert_eq!(accepted.bundle.ark.expose_bytes(), ark.expose_bytes());
        assert_eq!(accepted.bundle.account_id, id);
        assert_eq!(accepted.bundle.key_epoch, 0);
        assert_eq!(
            accepted.approver_device_id.as_deref(),
            Some(dev_a.device_id())
        );
    }

    #[test]
    fn accept_without_approval_is_not_found() {
        let relay = MemoryRelay::new();
        let (_ark, id) = account();
        let dev_b = DeviceKeypairs::generate().unwrap();
        enroll_publish(&relay, &id, &dev_b, NOW).unwrap();
        assert!(matches!(
            accept(&relay, &id, &dev_b),
            Err(SyncError::NotFound(_))
        ));
    }

    #[test]
    fn revoke_rewraps_to_remaining_and_excludes_revoked() {
        let relay = MemoryRelay::new();
        let (ark, id) = account();
        let dev_a = DeviceKeypairs::generate().unwrap();
        let dev_b = DeviceKeypairs::generate().unwrap();
        let dev_c = DeviceKeypairs::generate().unwrap();
        bootstrap(&relay, &id, &dev_a, 0, NOW).unwrap();
        enroll_publish(&relay, &id, &dev_b, NOW).unwrap();
        enroll_publish(&relay, &id, &dev_c, NOW).unwrap();

        let rev = revoke(&relay, &id, &dev_a, &ark, 0, dev_c.device_id(), NOW).unwrap();
        assert_eq!(rev.new_epoch, 1);
        assert!(
            rev.rewrapped_devices
                .contains(&dev_a.device_id().to_owned())
        );
        assert!(
            rev.rewrapped_devices
                .contains(&dev_b.device_id().to_owned())
        );
        assert!(
            !rev.rewrapped_devices
                .contains(&dev_c.device_id().to_owned())
        );

        // The revoked device receives no re-wrap for the new epoch.
        let c_wraps = relay
            .wraps_list(&id.to_hex(), dev_c.device_id(), 0)
            .unwrap();
        assert!(c_wraps.is_empty());
        // A revocation attestation is on the roster history.
        let atts = relay.attestations_list(&id.to_hex(), 0).unwrap();
        assert!(atts.iter().any(|a| {
            SignedAttestation::from_bytes(&a.attestation).is_ok_and(|s| {
                s.attestation.kind == AttestationKind::Revoke
                    && s.attestation.subject_device_id == dev_c.device_id()
            })
        }));
    }

    #[test]
    fn recovery_roundtrips_through_relay() {
        let relay = MemoryRelay::new();
        let (ark, id) = account();
        recovery_publish(&relay, &id, &ark, b"correct horse battery staple").unwrap();
        let recovered = recover_ark(&relay, &id, b"correct horse battery staple").unwrap();
        assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
        // Wrong secret fails.
        assert!(recover_ark(&relay, &id, b"nope").is_err());
    }

    #[test]
    fn recovery_absent_is_not_found() {
        let relay = MemoryRelay::new();
        let (_ark, id) = account();
        assert!(matches!(
            recover_ark(&relay, &id, b"x"),
            Err(SyncError::NotFound(_))
        ));
    }

    #[test]
    fn current_epoch_tracks_the_latest_attestation() {
        let relay = MemoryRelay::new();
        let (ark, id) = account();
        let dev_a = DeviceKeypairs::generate().unwrap();
        let dev_b = DeviceKeypairs::generate().unwrap();
        // No attestations yet: the account is at epoch 0.
        assert_eq!(current_epoch(&relay, &id).unwrap(), 0);

        bootstrap(&relay, &id, &dev_a, 0, NOW).unwrap();
        enroll_publish(&relay, &id, &dev_b, NOW).unwrap();
        assert_eq!(current_epoch(&relay, &id).unwrap(), 0);

        // Revoking B advances the epoch and attests it.
        revoke(&relay, &id, &dev_a, &ark, 0, dev_b.device_id(), NOW).unwrap();
        assert_eq!(current_epoch(&relay, &id).unwrap(), 1);
    }
}
