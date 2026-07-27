// SPDX-License-Identifier: Apache-2.0

//! The encryption glue between the engine and `pergamon-crypto` (ADR-024,
//! ADR-030).
//!
//! A [`CryptoContext`] holds the account key material and device identity for a
//! sync session and turns a plaintext [`ChangeBody`] into an encrypted, **signed**
//! [`EventInput`] (and back), binding the server-visible header — including the
//! origin `device_id` and blinded `entity_ref` (ADR-030) — as AEAD associated
//! data, blinding the `entity_ref`, signing the event with the device's Ed25519
//! key, and performing convergent blob encryption/decryption.
//!
//! A [`DeviceKeyDirectory`] maps opaque `device_id`s to their Ed25519 public
//! keys so the engine can verify each pulled event's signature; callers build it
//! from a verified device roster.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use pergamon_core::sync::event::ChangeBody;
use pergamon_crypto::hierarchy::{AccountContentKey, AccountRootKey};
use pergamon_crypto::primitives::{KEY_LEN, SIG_LEN};
use pergamon_crypto::{
    EncryptedBlob, EventHeader, SignedDeviceRecord, decrypt_blob, decrypt_event, encrypt_blob,
    encrypt_event, entity_ref, sign_event, verify_event,
};

use crate::error::{Result, SyncError};
use crate::wire::{EventInput, PROTOCOL_VERSION, StoredEvent};

/// A `device_id -> Ed25519 public key` directory the engine uses to verify the
/// signature on each pulled event (ADR-030).
///
/// Because a `device_id` is a hash of the device's Ed25519 public key, the
/// public key cannot be recovered from the handle alone; the caller builds this
/// from a **verified** device roster (e.g. [`from_roster`]) and passes it to the
/// engine. Keeping it a plain map keeps the engine transport-generic and
/// unit-testable with in-memory doubles.
///
/// [`from_roster`]: DeviceKeyDirectory::from_roster
#[derive(Debug, Clone, Default)]
pub struct DeviceKeyDirectory {
    keys: HashMap<String, [u8; KEY_LEN]>,
}

impl DeviceKeyDirectory {
    /// Create an empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a device's Ed25519 public key.
    pub fn insert(&mut self, device_id: impl Into<String>, ed25519_pub: [u8; KEY_LEN]) {
        self.keys.insert(device_id.into(), ed25519_pub);
    }

    /// Look up a device's Ed25519 public key, if known.
    #[must_use]
    pub fn get(&self, device_id: &str) -> Option<&[u8; KEY_LEN]> {
        self.keys.get(device_id)
    }

    /// Whether the directory knows no devices.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Number of known devices.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Build a directory from a set of **already-verified** signed device
    /// records (e.g. [`crate::onboarding::roster`]). Each record's self-signature
    /// and `device_id`↔key binding are assumed to have been checked by the
    /// caller; this only projects `device_id -> ed25519_pub`.
    #[must_use]
    pub fn from_roster(records: &[SignedDeviceRecord]) -> Self {
        let mut dir = Self::new();
        for r in records {
            dir.insert(r.record.device_id.clone(), r.record.ed25519_pub);
        }
        dir
    }
}

/// Per-session account key material and device identity.
///
/// Cheap to build from the persisted account root key; derives the content key
/// for the active epoch and the stream key used to blind entity references.
pub struct CryptoContext {
    ark: AccountRootKey,
    /// Opaque account handle (hex), as it appears on the wire.
    pub account_id_hex: String,
    /// Opaque origin-device handle.
    pub device_id: String,
    /// This device's Ed25519 signing-key seed, used to sign outgoing events
    /// (ADR-030). Never leaves the client.
    ed25519_signing: [u8; KEY_LEN],
    /// The active key epoch new events are encrypted under.
    pub key_epoch: u32,
    /// Cached HMAC stream key for `entity_ref` blinding.
    stream_key: [u8; 32],
}

impl CryptoContext {
    /// Build a context from the account root key, identity, device signing key,
    /// and active epoch.
    ///
    /// `ed25519_signing_key` is the device's Ed25519 signing-key seed (the same
    /// one behind `device_id`); it signs every outgoing event so peers can prove
    /// authorship independent of who holds the account content key (ADR-030).
    ///
    /// # Errors
    /// Returns a [`SyncError::Crypto`] if the stream key cannot be derived.
    pub fn new(
        ark: AccountRootKey,
        account_id_hex: String,
        device_id: String,
        ed25519_signing_key: [u8; KEY_LEN],
        key_epoch: u32,
    ) -> Result<Self> {
        let stream = ark.account_stream_key()?;
        let stream_key = *stream;
        Ok(Self {
            ark,
            account_id_hex,
            device_id,
            ed25519_signing: ed25519_signing_key,
            key_epoch,
            stream_key,
        })
    }

    /// This device's Ed25519 verifying key (derived from its signing seed).
    #[must_use]
    pub fn device_ed25519_pub(&self) -> [u8; KEY_LEN] {
        pergamon_crypto::primitives::ed25519_public(&self.ed25519_signing)
    }

    /// The account content key for the active epoch.
    fn content_key(&self) -> Result<AccountContentKey> {
        Ok(self.ark.content_key(self.key_epoch)?)
    }

    /// The account content key for a specific epoch (for decrypting old events).
    fn content_key_for(&self, epoch: u32) -> Result<AccountContentKey> {
        Ok(self.ark.content_key(epoch)?)
    }

    /// Encrypt and sign a change body into a wire [`EventInput`] under this
    /// device and the active epoch. `change_id` is the outbox row's idempotency
    /// key.
    ///
    /// The blinded `entity_ref` and this device's `device_id` are both bound into
    /// the header AAD, and the whole event is signed with the device's Ed25519
    /// key (ADR-030), so a hostile server can neither re-attribute nor re-route
    /// the body, and a revoked device's forgeries are detectable.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if serialization or encryption fails.
    pub fn encrypt_change(&self, change_id: &str, body: &ChangeBody) -> Result<EventInput> {
        let plaintext = body.to_bytes()?;
        let blob_refs = body.blob_refs();
        let blinded = entity_ref(&self.stream_key, body.entity_type.as_str(), &body.entity_id)?;
        let header = EventHeader {
            protocol_version: PROTOCOL_VERSION,
            account_id: self.account_id_hex.clone(),
            device_id: self.device_id.clone(),
            change_id: change_id.to_owned(),
            key_epoch: self.key_epoch,
            entity_ref: Some(blinded.clone()),
            blob_refs: blob_refs.clone(),
        };
        let ack = self.content_key()?;
        let ciphertext = encrypt_event(&ack, &header, &plaintext)?;
        let signature = sign_event(&self.ed25519_signing, &header, &ciphertext);
        Ok(EventInput {
            protocol_version: PROTOCOL_VERSION,
            account_id: self.account_id_hex.clone(),
            device_id: self.device_id.clone(),
            change_id: change_id.to_owned(),
            entity_ref: Some(blinded),
            key_epoch: self.key_epoch,
            blob_refs,
            ciphertext_b64: STANDARD.encode(&ciphertext),
            sig_b64: STANDARD.encode(signature),
        })
    }

    /// Reconstruct the AEAD header for a pulled event, mirroring exactly the
    /// header the producer bound at encrypt time (including its `device_id` and
    /// blinded `entity_ref`), so decryption re-binds the routing.
    fn header_for(event: &StoredEvent) -> EventHeader {
        EventHeader {
            protocol_version: event.protocol_version,
            account_id: event.account_id.clone(),
            device_id: event.device_id.clone(),
            change_id: event.change_id.clone(),
            key_epoch: event.key_epoch,
            entity_ref: event.entity_ref.clone(),
            blob_refs: event.blob_refs.clone(),
        }
    }

    /// Verify a pulled event's Ed25519 signature against the signer's public key
    /// (ADR-030). Returns `Ok(true)` when the signature authenticates the exact
    /// header + ciphertext, `Ok(false)` when it does not (bad/empty/malformed
    /// signature).
    ///
    /// # Errors
    /// Returns a [`SyncError::Base64`] if the ciphertext or signature base64 is
    /// malformed, or a [`SyncError::Crypto`] if the public key is not a valid
    /// point.
    pub fn verify_event_sig(
        &self,
        event: &StoredEvent,
        signer_ed25519_pub: &[u8; KEY_LEN],
    ) -> Result<bool> {
        let ciphertext = STANDARD.decode(&event.ciphertext_b64)?;
        let sig_bytes = STANDARD.decode(&event.sig_b64)?;
        let Ok(sig) = <[u8; SIG_LEN]>::try_from(sig_bytes.as_slice()) else {
            // An empty or wrong-length signature is simply an invalid signature.
            return Ok(false);
        };
        let header = Self::header_for(event);
        match verify_event(signer_ed25519_pub, &header, &ciphertext, &sig) {
            Ok(()) => Ok(true),
            Err(pergamon_crypto::CryptoError::BadSignature) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Decrypt a pulled [`StoredEvent`] back into its plaintext [`ChangeBody`],
    /// re-binding the echoed header (including `device_id` and `entity_ref`) as
    /// AAD.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if base64 decode, decryption, or parsing fails.
    pub fn decrypt_change(&self, event: &StoredEvent) -> Result<ChangeBody> {
        let ciphertext = STANDARD.decode(&event.ciphertext_b64)?;
        let header = Self::header_for(event);
        let ack = self.content_key_for(event.key_epoch)?;
        let plaintext = decrypt_event(&ack, &header, &ciphertext)?;
        Ok(ChangeBody::from_bytes(&plaintext)?)
    }

    /// Convergently encrypt a blob's plaintext for upload.
    ///
    /// # Errors
    /// Returns a [`SyncError::Crypto`] if encryption fails.
    pub fn encrypt_blob_plaintext(&self, plaintext: &[u8]) -> Result<EncryptedBlob> {
        Ok(encrypt_blob(&self.content_key()?, plaintext)?)
    }

    /// Decrypt a downloaded blob given the producer's `plaintext_hash`.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if the hex hash is malformed or decryption fails.
    pub fn decrypt_blob_ciphertext(
        &self,
        epoch: u32,
        plaintext_hash_hex: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>> {
        let hash = hex32(plaintext_hash_hex).ok_or_else(|| {
            SyncError::Protocol(format!("bad plaintext hash {plaintext_hash_hex}"))
        })?;
        Ok(decrypt_blob(
            &self.content_key_for(epoch)?,
            &hash,
            ciphertext,
        )?)
    }
}

/// Parse a 64-char lowercase-hex string into a 32-byte array.
fn hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
