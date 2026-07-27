// SPDX-License-Identifier: Apache-2.0

//! The [`Transport`] abstraction plus an in-memory test double.
//!
//! The engine talks to the sync server only through this trait, so it can be
//! driven over real HTTP (the `http` feature) or over an in-process
//! [`MemoryTransport`] that faithfully models the server's append-log, per-event
//! `change_id` idempotency, monotonic `server_seq`, cursor semantics, and blob
//! store — which is exactly what the convergence tests need.
#![allow(clippy::significant_drop_tightening)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::{Result, SyncError};
use crate::wire::PullResponse;
use crate::wire::{
    BlobProbeRequest, BlobProbeResponse, PushRequest, PushResponse, PushResult, StoredEvent,
};

/// A sync transport: the five ADR-022 operations the engine needs.
pub trait Transport {
    /// Append a batch of events to an account's log (idempotent per `change_id`).
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn push(&self, req: &PushRequest) -> Result<PushResponse>;

    /// Pull events with `server_seq > after`, ascending, up to `limit`.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn pull(&self, account_id: &str, after: u64, limit: Option<u32>) -> Result<PullResponse>;

    /// Ask which of `ct_hashes` the server is missing (upload only those).
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn blob_probe(&self, req: &BlobProbeRequest) -> Result<BlobProbeResponse>;

    /// Upload an opaque blob ciphertext under its `ct_hash` (idempotent).
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn blob_put(&self, account_id: &str, ct_hash: &str, ciphertext: &[u8]) -> Result<()>;

    /// Download an opaque blob ciphertext by `ct_hash`.
    ///
    /// # Errors
    /// Returns a [`SyncError::MissingBlob`] if absent, or [`SyncError::Transport`].
    fn blob_get(&self, account_id: &str, ct_hash: &str) -> Result<Vec<u8>>;
}

/// The append-log state of one account inside a [`MemoryTransport`].
#[derive(Debug, Default)]
struct AccountLog {
    /// Committed events in server order.
    events: Vec<StoredEvent>,
    /// `change_id` -> assigned `server_seq`, for idempotent re-push.
    by_change: HashMap<String, u64>,
    /// Opaque blob ciphertext by `ct_hash`.
    blobs: HashMap<String, Vec<u8>>,
    /// Next `server_seq` to assign (1-based).
    next_seq: u64,
}

/// An in-process [`Transport`] double that models the sync server closely enough
/// to drive real convergence tests. Cloneable and thread-safe: clone it to give
/// several engines a shared "server".
#[derive(Debug, Clone, Default)]
pub struct MemoryTransport {
    accounts: Arc<Mutex<HashMap<String, AccountLog>>>,
}

impl MemoryTransport {
    /// Create an empty in-memory transport.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of committed events for an account (test introspection).
    ///
    /// # Panics
    /// Panics only if the internal lock is poisoned by a prior panic.
    #[must_use]
    pub fn event_count(&self, account_id: &str) -> usize {
        self.accounts
            .lock()
            .map_or(0, |m| m.get(account_id).map_or(0, |a| a.events.len()))
    }

    fn locked(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, AccountLog>>> {
        self.accounts
            .lock()
            .map_err(|_| SyncError::Transport("memory transport poisoned".to_owned()))
    }
}

impl Transport for MemoryTransport {
    fn push(&self, req: &PushRequest) -> Result<PushResponse> {
        let mut accounts = self.locked()?;
        let log = accounts.entry(req.account_id.clone()).or_default();
        if log.next_seq == 0 {
            log.next_seq = 1;
        }
        let mut results = Vec::with_capacity(req.events.len());
        for ev in &req.events {
            if ev.account_id != req.account_id {
                return Err(SyncError::Protocol(
                    "event account_id does not match batch".to_owned(),
                ));
            }
            if let Some(&seq) = log.by_change.get(&ev.change_id) {
                results.push(PushResult {
                    change_id: ev.change_id.clone(),
                    server_seq: seq,
                    deduplicated: true,
                });
                continue;
            }
            let seq = log.next_seq;
            log.next_seq += 1;
            let payload_bytes = ev.ciphertext_b64.len() as u64;
            log.events.push(StoredEvent {
                protocol_version: ev.protocol_version,
                account_id: ev.account_id.clone(),
                device_id: ev.device_id.clone(),
                change_id: ev.change_id.clone(),
                entity_ref: ev.entity_ref.clone(),
                key_epoch: ev.key_epoch,
                blob_refs: ev.blob_refs.clone(),
                payload_bytes,
                server_seq: seq,
                server_committed_at: 0,
                ciphertext_b64: ev.ciphertext_b64.clone(),
                sig_b64: ev.sig_b64.clone(),
            });
            log.by_change.insert(ev.change_id.clone(), seq);
            results.push(PushResult {
                change_id: ev.change_id.clone(),
                server_seq: seq,
                deduplicated: false,
            });
        }
        let high_water_seq = log.next_seq.saturating_sub(1);
        Ok(PushResponse {
            results,
            high_water_seq,
        })
    }

    fn pull(&self, account_id: &str, after: u64, limit: Option<u32>) -> Result<PullResponse> {
        let accounts = self.locked()?;
        let Some(log) = accounts.get(account_id) else {
            return Ok(PullResponse {
                events: Vec::new(),
                high_water_seq: 0,
                next_cursor: after,
            });
        };
        let cap = limit.map_or(usize::MAX, |l| l as usize);
        let events: Vec<StoredEvent> = log
            .events
            .iter()
            .filter(|e| e.server_seq > after)
            .take(cap)
            .cloned()
            .collect();
        let high_water_seq = log.next_seq.saturating_sub(1);
        let next_cursor = events.last().map_or(after, |e| e.server_seq);
        Ok(PullResponse {
            events,
            high_water_seq,
            next_cursor,
        })
    }

    fn blob_probe(&self, req: &BlobProbeRequest) -> Result<BlobProbeResponse> {
        let accounts = self.locked()?;
        let empty = AccountLog::default();
        let log = accounts.get(&req.account_id).unwrap_or(&empty);
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for h in &req.ct_hashes {
            if log.blobs.contains_key(h) {
                present.push(h.clone());
            } else {
                missing.push(h.clone());
            }
        }
        Ok(BlobProbeResponse { present, missing })
    }

    fn blob_put(&self, account_id: &str, ct_hash: &str, ciphertext: &[u8]) -> Result<()> {
        let mut accounts = self.locked()?;
        let log = accounts.entry(account_id.to_owned()).or_default();
        log.blobs.insert(ct_hash.to_owned(), ciphertext.to_vec());
        Ok(())
    }

    fn blob_get(&self, account_id: &str, ct_hash: &str) -> Result<Vec<u8>> {
        let accounts = self.locked()?;
        accounts
            .get(account_id)
            .and_then(|log| log.blobs.get(ct_hash).cloned())
            .ok_or_else(|| SyncError::MissingBlob(ct_hash.to_owned()))
    }
}
