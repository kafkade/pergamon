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
}

/// A sync result.
pub type Result<T> = std::result::Result<T, SyncError>;
