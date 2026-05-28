//! Import parsing errors.

/// Errors that may occur while parsing import files.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The CSV data could not be parsed.
    #[error("CSV parse error: {0}")]
    Csv(#[from] csv::Error),

    /// The HTML data could not be parsed.
    #[error("HTML parse error: {0}")]
    Html(String),

    /// A timestamp could not be parsed.
    #[error("invalid timestamp: {0}")]
    Timestamp(String),

    /// UTF-8 decoding failed.
    #[error("invalid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    /// UTF-8 decoding failed (str variant).
    #[error("invalid UTF-8: {0}")]
    Utf8Str(#[from] std::str::Utf8Error),
}
