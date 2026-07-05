// SPDX-License-Identifier: Apache-2.0

//! The encryption glue between the engine and `pergamon-crypto` (ADR-024).
//!
//! A [`CryptoContext`] holds the account key material and device identity for a
//! sync session and turns a plaintext [`ChangeBody`] into an encrypted
//! [`EventInput`] (and back), binding the server-visible header as AEAD
//! associated data, blinding the `entity_ref`, and performing convergent blob
//! encryption/decryption.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use pergamon_core::sync::event::ChangeBody;
use pergamon_crypto::hierarchy::{AccountContentKey, AccountRootKey};
use pergamon_crypto::{
    EncryptedBlob, EventHeader, decrypt_blob, decrypt_event, encrypt_blob, encrypt_event,
    entity_ref,
};

use crate::error::{Result, SyncError};
use crate::wire::{EventInput, PROTOCOL_VERSION, StoredEvent};

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
    /// The active key epoch new events are encrypted under.
    pub key_epoch: u32,
    /// Cached HMAC stream key for `entity_ref` blinding.
    stream_key: [u8; 32],
}

impl CryptoContext {
    /// Build a context from the account root key, identity, and active epoch.
    ///
    /// # Errors
    /// Returns a [`SyncError::Crypto`] if the stream key cannot be derived.
    pub fn new(
        ark: AccountRootKey,
        account_id_hex: String,
        device_id: String,
        key_epoch: u32,
    ) -> Result<Self> {
        let stream = ark.account_stream_key()?;
        let stream_key = *stream;
        Ok(Self {
            ark,
            account_id_hex,
            device_id,
            key_epoch,
            stream_key,
        })
    }

    /// The account content key for the active epoch.
    fn content_key(&self) -> Result<AccountContentKey> {
        Ok(self.ark.content_key(self.key_epoch)?)
    }

    /// The account content key for a specific epoch (for decrypting old events).
    fn content_key_for(&self, epoch: u32) -> Result<AccountContentKey> {
        Ok(self.ark.content_key(epoch)?)
    }

    /// Encrypt a change body into a wire [`EventInput`] under this device and
    /// the active epoch. `change_id` is the outbox row's idempotency key.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if serialization or encryption fails.
    pub fn encrypt_change(&self, change_id: &str, body: &ChangeBody) -> Result<EventInput> {
        let plaintext = body.to_bytes()?;
        let blob_refs = body.blob_refs();
        let header = EventHeader {
            protocol_version: PROTOCOL_VERSION,
            account_id: self.account_id_hex.clone(),
            change_id: change_id.to_owned(),
            key_epoch: self.key_epoch,
            blob_refs: blob_refs.clone(),
        };
        let ack = self.content_key()?;
        let ciphertext = encrypt_event(&ack, &header, &plaintext)?;
        let blinded = entity_ref(&self.stream_key, body.entity_type.as_str(), &body.entity_id)?;
        Ok(EventInput {
            protocol_version: PROTOCOL_VERSION,
            account_id: self.account_id_hex.clone(),
            device_id: self.device_id.clone(),
            change_id: change_id.to_owned(),
            entity_ref: Some(blinded),
            key_epoch: self.key_epoch,
            blob_refs,
            ciphertext_b64: STANDARD.encode(&ciphertext),
        })
    }

    /// Decrypt a pulled [`StoredEvent`] back into its plaintext [`ChangeBody`],
    /// re-binding the echoed header as AAD.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if base64 decode, decryption, or parsing fails.
    pub fn decrypt_change(&self, event: &StoredEvent) -> Result<ChangeBody> {
        let ciphertext = STANDARD.decode(&event.ciphertext_b64)?;
        let header = EventHeader {
            protocol_version: event.protocol_version,
            account_id: event.account_id.clone(),
            change_id: event.change_id.clone(),
            key_epoch: event.key_epoch,
            blob_refs: event.blob_refs.clone(),
        };
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
