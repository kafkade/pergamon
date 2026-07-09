// SPDX-License-Identifier: AGPL-3.0-only

//! Background remote-sync worker for the web server (issue #129).
//!
//! When the operator configures a sync account and an encrypted key file, this
//! module spawns a dedicated OS thread that drives
//! [`pergamon_sync::run_forever`]: it opens its **own** `SQLite` connection to the
//! same database file (WAL mode makes cross-connection writes safe, so a
//! network-bound sync round never holds the request-handler mutex), builds the
//! ADR-024 crypto context from the shared [`pergamon_keystore`], and runs
//! repeated push+pull rounds on an interval with exponential backoff on
//! offline/transient failures.
//!
//! The returned [`pergamon_sync::SyncControl`] lets request handlers trigger an
//! out-of-band round (`POST /admin/sync-remote/trigger`) and lets `main` stop
//! the worker on shutdown.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use pergamon_storage::Database;

/// Operator-supplied configuration for the background sync worker.
pub struct SyncWorkerConfig {
    /// Path to the `SQLite` database file (the worker opens its own connection).
    pub db_path: PathBuf,
    /// Account handle whose keys back this sync (as stored in the key file).
    pub account: String,
    /// Path to the Argon2id-encrypted key file holding the account root key.
    pub key_file: PathBuf,
    /// Passphrase that unlocks the key file.
    pub passphrase: String,
    /// Seconds between successful sync rounds.
    pub interval_secs: u64,
}

/// Base backoff delay after the first offline/transient failure.
const BACKOFF_BASE: Duration = Duration::from_secs(5);
/// Ceiling the backoff delay is clamped to (5 minutes).
#[allow(clippy::duration_suboptimal_units)]
const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// Backoff growth factor per consecutive failure.
const BACKOFF_MULTIPLIER: f64 = 2.0;

/// Start the background sync worker, returning a control handle.
///
/// # Errors
/// Returns an error if remote sync is not enabled on the database, if the key
/// file cannot be unlocked, or if the crypto/transport context cannot be built.
pub fn spawn(config: SyncWorkerConfig) -> Result<pergamon_sync::SyncControl> {
    let SyncWorkerConfig {
        db_path,
        account,
        key_file,
        passphrase,
        interval_secs,
    } = config;

    // A dedicated connection: the worker must not contend with request handlers
    // for the shared `AppState` mutex during network I/O.
    let db = Database::open(&db_path)
        .with_context(|| format!("opening sync-worker database at {}", db_path.display()))?;

    let state = db.sync_state().context("reading sync state")?;
    let Some(server) = state.server_url.clone() else {
        bail!(
            "remote sync is not enabled on this database; run \
             `pergamon sync-remote enable --server <url>` first"
        );
    };
    let account_hex = state
        .account_id
        .clone()
        .ok_or_else(|| anyhow!("sync identity is incomplete (no account id)"))?;
    let device_id = state
        .device_id
        .clone()
        .ok_or_else(|| anyhow!("sync identity is incomplete (no device id)"))?;

    let store =
        pergamon_keystore::DeviceKeyStore::encrypted_file(key_file.clone(), passphrase.as_bytes())
            .with_context(|| format!("unlocking key file {}", key_file.display()))?;
    let ark = store
        .load_ark(&account)?
        .ok_or_else(|| anyhow!("no account root key for '{account}' in the key file"))?;

    let crypto = pergamon_sync::CryptoContext::new(ark, account_hex, device_id, state.key_epoch)
        .context("building crypto context")?;
    let transport = pergamon_sync::http::HttpTransport::new(server.clone())
        .context("building HTTP transport")?;
    let engine = pergamon_sync::SyncEngine::new(transport, crypto);
    let blobs = pergamon_sync::MemoryBlobStore::new();

    let interval = Duration::from_secs(interval_secs.max(1));
    let backoff = pergamon_sync::BackoffPolicy::new(BACKOFF_BASE, BACKOFF_MAX, BACKOFF_MULTIPLIER);
    let scheduler = pergamon_sync::SyncScheduler::new(interval, backoff);
    let (control, sleeper) = pergamon_sync::control();

    std::thread::Builder::new()
        .name("pergamon-sync-worker".to_owned())
        .spawn(move || {
            let result = pergamon_sync::run_forever(
                || engine.sync(&db, &blobs),
                scheduler,
                &sleeper,
                pergamon_sync::Jitter::from_entropy(),
                log_round,
            );
            match result {
                Ok(()) => tracing::info!("background sync worker stopped"),
                Err(e) => tracing::error!("background sync worker stopped on fatal error: {e}"),
            }
        })
        .context("spawning sync worker thread")?;

    tracing::info!(
        server = %server,
        interval_secs,
        "background sync worker started"
    );
    Ok(control)
}

/// Log the result of one background sync round.
fn log_round(report: &pergamon_sync::RoundReport) {
    match &report.outcome {
        pergamon_sync::RoundOutcome::Synced(stats) => {
            if stats.pushed > 0 || stats.applied > 0 {
                tracing::info!(
                    pushed = stats.pushed,
                    applied = stats.applied,
                    next_secs = report.next_delay.as_secs(),
                    "sync round complete"
                );
            } else {
                tracing::debug!(
                    next_secs = report.next_delay.as_secs(),
                    "sync round complete (no changes)"
                );
            }
        }
        pergamon_sync::RoundOutcome::Offline(msg) => {
            tracing::warn!(
                consecutive_failures = report.consecutive_failures,
                retry_secs = report.next_delay.as_secs(),
                "sync round offline: {msg}"
            );
        }
    }
}
