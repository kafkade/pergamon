//! Error types for pergamon-storage.

use thiserror::Error;

/// Errors produced by the storage layer.
#[derive(Debug, Error)]
pub enum StorageError {
    /// An error from the underlying `SQLite` connection.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A domain value could not be parsed from the database.
    #[error("domain error: {0}")]
    Domain(#[from] pergamon_core::error::CoreError),

    /// An entity was not found.
    #[error("{entity} not found: {id}")]
    NotFound {
        /// The entity kind (e.g. `content_item`, `feed`).
        entity: &'static str,
        /// The ID that was looked up.
        id: String,
    },

    /// A generic storage-layer error.
    #[error("{0}")]
    Generic(String),
}
