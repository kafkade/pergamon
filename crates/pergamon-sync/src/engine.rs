// SPDX-License-Identifier: Apache-2.0

//! The [`SyncEngine`]: the push/pull orchestration that ties storage, crypto,
//! transport, and the merge policy together.
//!
//! - [`SyncEngine::push`] drains the local outbox, uploads referenced blobs
//!   (upload-before-commit), encrypts each change, appends the batch, and acks
//!   the outbox rows.
//! - [`SyncEngine::pull`] fetches events past the cursor, suppresses this
//!   device's echoes, decrypts, fetches referenced blobs, applies through the
//!   merge policy inside a transaction, records idempotency, and advances the
//!   cursor.
//! - [`SyncEngine::sync`] runs a push then a pull.

use std::time::{SystemTime, UNIX_EPOCH};

use pergamon_core::sync::event::ChangeBody;
use pergamon_storage::Database;

use crate::apply::apply_change;
use crate::blob::BlobStore;
use crate::crypto::CryptoContext;
use crate::error::{Result, SyncError};
use crate::transport::Transport;
use crate::wire::{BlobProbeRequest, EventInput, PushRequest};

/// How many events to move per push/pull batch.
const BATCH: usize = 256;

/// Counters describing the work one sync round performed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Events pushed to the server (excludes server-side dedup hits).
    pub pushed: usize,
    /// Events pulled and applied locally (excludes echoes and re-applies).
    pub applied: usize,
}

impl SyncStats {
    /// Combine two rounds' stats.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        Self {
            pushed: self.pushed + other.pushed,
            applied: self.applied + other.applied,
        }
    }
}

/// The client sync engine, generic over a [`Transport`].
pub struct SyncEngine<T: Transport> {
    transport: T,
    crypto: CryptoContext,
}

impl<T: Transport> SyncEngine<T> {
    /// Build an engine from a transport and account crypto context.
    pub const fn new(transport: T, crypto: CryptoContext) -> Self {
        Self { transport, crypto }
    }

    /// Borrow the underlying transport (test introspection).
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// Push all pending local changes, uploading referenced blobs first.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if a blob is missing, or if encryption, the
    /// transport, or storage fail.
    pub fn push(&self, db: &Database, blobs: &dyn BlobStore) -> Result<usize> {
        let account = self.crypto.account_id_hex.clone();
        let mut pushed = 0usize;
        loop {
            let pending = db.pending_outbox(BATCH)?;
            if pending.is_empty() {
                break;
            }
            let mut events: Vec<EventInput> = Vec::with_capacity(pending.len());
            for row in &pending {
                let body = ChangeBody::from_bytes(&row.body)?;
                self.upload_blobs(&account, &body, blobs)?;
                events.push(self.crypto.encrypt_change(&row.change_id, &body)?);
            }
            let req = PushRequest {
                account_id: account.clone(),
                events,
            };
            let resp = self.transport.push(&req)?;
            for result in &resp.results {
                db.mark_outbox_acked(&result.change_id, result.server_seq)?;
                if !result.deduplicated {
                    pushed += 1;
                }
            }
        }
        Ok(pushed)
    }

    /// Upload every blob a change references that the server is missing.
    fn upload_blobs(&self, account: &str, body: &ChangeBody, blobs: &dyn BlobStore) -> Result<()> {
        if body.blob_manifest.is_empty() {
            return Ok(());
        }
        let ct_hashes: Vec<String> = body
            .blob_manifest
            .iter()
            .map(|b| b.ct_hash.clone())
            .collect();
        let probe = self.transport.blob_probe(&BlobProbeRequest {
            account_id: account.to_owned(),
            ct_hashes,
        })?;
        for entry in &body.blob_manifest {
            if !probe.missing.contains(&entry.ct_hash) {
                continue;
            }
            let plaintext = blobs
                .load(&entry.plaintext_hash)
                .map_err(SyncError::Transport)?
                .ok_or_else(|| SyncError::MissingBlob(entry.ct_hash.clone()))?;
            let encrypted = self.crypto.encrypt_blob_plaintext(&plaintext)?;
            self.transport
                .blob_put(account, &encrypted.ct_hash, &encrypted.ciphertext)?;
        }
        Ok(())
    }

    /// Pull and apply all events past the local cursor.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if the transport, decryption, or storage fail.
    pub fn pull(&self, db: &Database, blobs: &dyn BlobStore) -> Result<usize> {
        let account = self.crypto.account_id_hex.clone();
        let mut applied = 0usize;
        loop {
            let cursor = db.sync_cursor()?;
            let page = self.transport.pull(
                &account,
                cursor,
                Some(u32::try_from(BATCH).unwrap_or(u32::MAX)),
            )?;
            if page.events.is_empty() {
                break;
            }
            let page_len = page.events.len();
            for ev in &page.events {
                // Suppress this device's own echoes.
                if ev.device_id == self.crypto.device_id {
                    continue;
                }
                if db.is_change_applied(&ev.change_id)? {
                    continue;
                }
                let body = self.crypto.decrypt_change(ev)?;
                self.fetch_blobs(&account, &body, ev.key_epoch, blobs)?;
                let server_seq = ev.server_seq;
                let change_id = ev.change_id.clone();
                db.in_transaction(|db| -> Result<()> {
                    db.observe_remote_hlc(&body.clock, now_millis())?;
                    apply_change(db, &body)?;
                    db.mark_change_applied(&change_id, server_seq)?;
                    Ok(())
                })?;
                applied += 1;
            }
            db.set_sync_cursor(page.next_cursor)?;
            if page_len < BATCH {
                break;
            }
        }
        Ok(applied)
    }

    /// Fetch and locally store every blob a change references.
    fn fetch_blobs(
        &self,
        account: &str,
        body: &ChangeBody,
        key_epoch: u32,
        blobs: &dyn BlobStore,
    ) -> Result<()> {
        for entry in &body.blob_manifest {
            let already = blobs
                .load(&entry.plaintext_hash)
                .map_err(SyncError::Transport)?
                .is_some();
            if already {
                continue;
            }
            let ciphertext = self.transport.blob_get(account, &entry.ct_hash)?;
            let plaintext = self.crypto.decrypt_blob_ciphertext(
                key_epoch,
                &entry.plaintext_hash,
                &ciphertext,
            )?;
            blobs
                .store(&entry.plaintext_hash, &plaintext)
                .map_err(SyncError::Transport)?;
        }
        Ok(())
    }

    /// Run a full sync round: push local changes, then pull remote ones.
    ///
    /// # Errors
    /// Returns a [`SyncError`] if either phase fails.
    pub fn sync(&self, db: &Database, blobs: &dyn BlobStore) -> Result<SyncStats> {
        let pushed = self.push(db, blobs)?;
        let applied = self.pull(db, blobs)?;
        Ok(SyncStats { pushed, applied })
    }

    /// Verify a push fully uploaded the local library (issue #184).
    ///
    /// A push is complete when the outbox is fully drained *and* every blob the
    /// outbox referenced is present on the server. This is the upload-completeness
    /// check that makes a failed or partial baseline loud instead of silent: it
    /// returns [`SyncError::IncompleteUpload`] listing what is still pending or
    /// missing, so callers (and tests) can assert the whole library landed.
    ///
    /// # Errors
    /// Returns [`SyncError::IncompleteUpload`] when changes remain pending or
    /// referenced blobs are still missing on the server, or a transport/storage
    /// error while probing.
    pub fn verify_upload_complete(&self, db: &Database) -> Result<()> {
        let pending_events = db.pending_outbox_count()?;
        let ct_hashes = db.outbox_blob_refs()?;
        let missing_blobs = if ct_hashes.is_empty() {
            Vec::new()
        } else {
            self.transport
                .blob_probe(&BlobProbeRequest {
                    account_id: self.crypto.account_id_hex.clone(),
                    ct_hashes,
                })?
                .missing
        };
        if pending_events == 0 && missing_blobs.is_empty() {
            Ok(())
        } else {
            Err(SyncError::IncompleteUpload {
                pending_events,
                missing_blobs,
            })
        }
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
