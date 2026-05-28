//! Error types for pergamon-core.

use thiserror::Error;

/// Top-level error type for the pergamon core library.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A content type string could not be parsed.
    #[error("unknown content type: {0}")]
    UnknownContentType(String),

    /// A document status string could not be parsed.
    #[error("unknown document status: {0}")]
    UnknownDocumentStatus(String),
}
