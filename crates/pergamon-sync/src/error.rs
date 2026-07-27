// SPDX-License-Identifier: Apache-2.0

//! Error type for the sync engine.

use thiserror::Error;

/// Errors produced while syncing.
#[derive(Debug, Error)]
pub enum SyncError {
    /// A storage-layer failure (outbox, clocks, canonical writes).
    #[error("storage error: {0}")]
    Storage(#[from] pergamon_storage::StorageError),

    /// An encryption or decryption failure.
    #[error("crypto error: {0}")]
    Crypto(#[from] pergamon_crypto::CryptoError),

    /// A (de)serialization failure for an event body or wire frame.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A base64 decode failure for a ciphertext body.
    #[error("base64 error: {0}")]
    Base64(#[from] base64::DecodeError),

    /// The transport (network or in-memory double) failed.
    #[error("transport error: {0}")]
    Transport(String),

    /// Sync is not configured (no account/device identity persisted).
    #[error("sync is not enabled: {0}")]
    NotEnabled(String),

    /// A referenced blob was missing from the local blob store or the server.
    #[error("missing blob: {0}")]
    MissingBlob(String),

    /// A relayed artifact (device record, wrap bundle, recovery blob) was not
    /// found on the server.
    #[error("not found: {0}")]
    NotFound(String),

    /// A protocol or data-shape invariant was violated.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A push did not fully upload the local library: some outbox changes are
    /// still pending and/or some referenced blobs are still missing on the
    /// server (issue #184). Signals an incomplete/partial baseline upload loudly.
    #[error(
        "upload incomplete: {pending_events} change(s) still pending, \
         {} blob(s) missing on server",
        missing_blobs.len()
    )]
    IncompleteUpload {
        /// Outbox changes still awaiting acknowledgement after the push.
        pending_events: u64,
        /// Referenced blob ciphertext hashes the server reports as still missing.
        missing_blobs: Vec<String>,
    },
}

impl SyncError {
    /// Whether this error is *transient* and worth retrying with backoff.
    ///
    /// Network failures, timeouts, and server-side errors surface as
    /// [`SyncError::Transport`]; a background sync loop should tolerate these
    /// (the device may simply be offline) and retry after a backoff rather than
    /// stop. A missing relayed artifact ([`SyncError::NotFound`]) is likewise
    /// treated as transient because the peer may not have uploaded it yet.
    ///
    /// Everything else — crypto, (de)serialization, base64, protocol shape,
    /// missing local blobs, or an unconfigured account — is a *fatal* condition
    /// that retrying cannot fix, so the caller should surface it instead of
    /// spinning.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::NotFound(_))
    }
}

/// A sync result.
pub type Result<T> = std::result::Result<T, SyncError>;
