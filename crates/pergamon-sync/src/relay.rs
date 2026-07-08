// SPDX-License-Identifier: Apache-2.0

//! The [`RelayTransport`] abstraction for ADR-024 onboarding artifacts, plus an
//! in-memory test double.
//!
//! The onboarding flows (bootstrap, enrollment, revocation, recovery) exchange
//! four kinds of **opaque** artifact through the sync server's blind relay:
//!
//! - signed **device records** (the account roster), keyed by `device_id`;
//! - sealed **key-wrap bundles** targeted at a recipient device, an append-only
//!   per-recipient sequence;
//! - signed **trust / revocation attestations**, an append-only per-account
//!   sequence; and
//! - the optional **recovery blob**, one per account.
//!
//! Every payload is ciphertext or a client signature the server cannot read; the
//! trait therefore trades only `&[u8]`. Like [`crate::Transport`], it can be
//! driven over real HTTP (the `http` feature) or the in-process [`MemoryRelay`]
//! the onboarding unit tests use.
#![allow(clippy::significant_drop_tightening)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::{Result, SyncError};

/// Hex-encode the BLAKE3 hash of `bytes`, for content-hash deduplication in the
/// in-memory relay (matching the server's dedup key domain).
fn blake3_hex(bytes: &[u8]) -> String {
    let digest = pergamon_crypto::primitives::blake3_hash(bytes);
    let mut s = String::with_capacity(64);
    for b in &digest {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    s
}

/// One device's roster entry: its opaque handle and serialized signed record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayDevice {
    /// The device's opaque handle.
    pub device_id: String,
    /// The serialized [`pergamon_crypto::SignedDeviceRecord`] bytes.
    pub record: Vec<u8>,
}

/// A sequenced, sealed key-wrap bundle addressed to a recipient device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayWrap {
    /// Per-recipient monotonic sequence (the pull cursor domain).
    pub seq: u64,
    /// The opaque sealed bundle bytes.
    pub bundle: Vec<u8>,
}

/// A sequenced, signed attestation in an account's roster history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAttestation {
    /// Per-account monotonic sequence (the pull cursor domain).
    pub seq: u64,
    /// The serialized [`pergamon_crypto::SignedAttestation`] bytes.
    pub attestation: Vec<u8>,
}

/// The blind-relay operations onboarding needs from the sync server.
///
/// All artifacts are opaque bytes: the server (and this trait) never interpret
/// them. Authenticity is enforced entirely client-side by verifying signatures
/// and opening sealed boxes.
pub trait RelayTransport {
    /// Publish (or replace) a device's opaque signed record.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn device_put(&self, account_id: &str, device_id: &str, record: &[u8]) -> Result<()>;

    /// Fetch one device's opaque record, or `None` if it has none.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn device_get(&self, account_id: &str, device_id: &str) -> Result<Option<Vec<u8>>>;

    /// List an account's full device roster.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn devices_list(&self, account_id: &str) -> Result<Vec<RelayDevice>>;

    /// Relay a sealed key-wrap bundle to a recipient device, returning its
    /// assigned (or pre-existing, on dedup) sequence.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn wrap_put(&self, account_id: &str, device_id: &str, bundle: &[u8]) -> Result<u64>;

    /// List a device's pending key-wrap bundles with `seq > after`, ascending.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn wraps_list(&self, account_id: &str, device_id: &str, after: u64) -> Result<Vec<RelayWrap>>;

    /// Append a signed attestation to an account's roster history, returning its
    /// assigned (or pre-existing, on dedup) sequence.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn attestation_append(&self, account_id: &str, attestation: &[u8]) -> Result<u64>;

    /// List an account's attestation history with `seq > after`, ascending.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn attestations_list(&self, account_id: &str, after: u64) -> Result<Vec<RelayAttestation>>;

    /// Store (or replace) an account's opaque recovery blob.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn recovery_put(&self, account_id: &str, blob: &[u8]) -> Result<()>;

    /// Fetch an account's opaque recovery blob, or `None` if recovery is off.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the underlying transport fails.
    fn recovery_get(&self, account_id: &str) -> Result<Option<Vec<u8>>>;
}

/// The relay state of one account inside a [`MemoryRelay`].
#[derive(Debug, Default)]
struct AccountRelay {
    /// Device records keyed by `device_id` (replace-on-put).
    devices: HashMap<String, Vec<u8>>,
    /// Per-recipient sealed wrap bundles: `device_id` -> sequenced bundles.
    wraps: HashMap<String, Vec<RelayWrap>>,
    /// Content hashes already stored per recipient, for idempotent re-put.
    wrap_hashes: HashMap<String, HashMap<String, u64>>,
    /// Per-account attestation history.
    attestations: Vec<RelayAttestation>,
    /// Content hashes already stored, for idempotent re-append.
    attestation_hashes: HashMap<String, u64>,
    /// The single recovery blob, if enabled.
    recovery: Option<Vec<u8>>,
}

/// An in-process [`RelayTransport`] double modelling the server's relay store.
///
/// It replaces device records and the recovery blob on put, and keeps
/// append-only, content-hash-deduplicated, per-cursor wraps and attestations.
/// Cloneable and thread-safe so several onboarding actors can share one
/// "server".
#[derive(Debug, Clone, Default)]
pub struct MemoryRelay {
    accounts: Arc<Mutex<HashMap<String, AccountRelay>>>,
}

impl MemoryRelay {
    /// Create an empty in-memory relay.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn locked(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, AccountRelay>>> {
        self.accounts
            .lock()
            .map_err(|_| SyncError::Transport("memory relay poisoned".to_owned()))
    }
}

impl RelayTransport for MemoryRelay {
    fn device_put(&self, account_id: &str, device_id: &str, record: &[u8]) -> Result<()> {
        let mut accounts = self.locked()?;
        let acct = accounts.entry(account_id.to_owned()).or_default();
        acct.devices.insert(device_id.to_owned(), record.to_vec());
        Ok(())
    }

    fn device_get(&self, account_id: &str, device_id: &str) -> Result<Option<Vec<u8>>> {
        let accounts = self.locked()?;
        Ok(accounts
            .get(account_id)
            .and_then(|a| a.devices.get(device_id).cloned()))
    }

    fn devices_list(&self, account_id: &str) -> Result<Vec<RelayDevice>> {
        let accounts = self.locked()?;
        let Some(acct) = accounts.get(account_id) else {
            return Ok(Vec::new());
        };
        let mut devices: Vec<RelayDevice> = acct
            .devices
            .iter()
            .map(|(id, bytes)| RelayDevice {
                device_id: id.clone(),
                record: bytes.clone(),
            })
            .collect();
        devices.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        Ok(devices)
    }

    fn wrap_put(&self, account_id: &str, device_id: &str, bundle: &[u8]) -> Result<u64> {
        let mut accounts = self.locked()?;
        let acct = accounts.entry(account_id.to_owned()).or_default();
        let hash = blake3_hex(bundle);
        let seen = acct.wrap_hashes.entry(device_id.to_owned()).or_default();
        if let Some(&seq) = seen.get(&hash) {
            return Ok(seq);
        }
        let list = acct.wraps.entry(device_id.to_owned()).or_default();
        let seq = list.last().map_or(1, |w| w.seq + 1);
        list.push(RelayWrap {
            seq,
            bundle: bundle.to_vec(),
        });
        acct.wrap_hashes
            .entry(device_id.to_owned())
            .or_default()
            .insert(hash, seq);
        Ok(seq)
    }

    fn wraps_list(&self, account_id: &str, device_id: &str, after: u64) -> Result<Vec<RelayWrap>> {
        let accounts = self.locked()?;
        Ok(accounts
            .get(account_id)
            .and_then(|a| a.wraps.get(device_id))
            .map(|list| {
                list.iter()
                    .filter(|w| w.seq > after)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    }

    fn attestation_append(&self, account_id: &str, attestation: &[u8]) -> Result<u64> {
        let mut accounts = self.locked()?;
        let acct = accounts.entry(account_id.to_owned()).or_default();
        let hash = blake3_hex(attestation);
        if let Some(&seq) = acct.attestation_hashes.get(&hash) {
            return Ok(seq);
        }
        let seq = acct.attestations.last().map_or(1, |a| a.seq + 1);
        acct.attestations.push(RelayAttestation {
            seq,
            attestation: attestation.to_vec(),
        });
        acct.attestation_hashes.insert(hash, seq);
        Ok(seq)
    }

    fn attestations_list(&self, account_id: &str, after: u64) -> Result<Vec<RelayAttestation>> {
        let accounts = self.locked()?;
        Ok(accounts
            .get(account_id)
            .map(|a| {
                a.attestations
                    .iter()
                    .filter(|x| x.seq > after)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    }

    fn recovery_put(&self, account_id: &str, blob: &[u8]) -> Result<()> {
        let mut accounts = self.locked()?;
        accounts.entry(account_id.to_owned()).or_default().recovery = Some(blob.to_vec());
        Ok(())
    }

    fn recovery_get(&self, account_id: &str) -> Result<Option<Vec<u8>>> {
        let accounts = self.locked()?;
        Ok(accounts.get(account_id).and_then(|a| a.recovery.clone()))
    }
}
