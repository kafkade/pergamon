//! Errors for the article extraction crate.

/// Errors that can occur during content extraction.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// HTML could not be parsed or article content could not be extracted.
    #[error("article extraction failed: {0}")]
    Extract(String),

    /// PDF text extraction failed.
    #[error("PDF text extraction failed: {0}")]
    Pdf(String),

    /// Input could not be decoded as UTF-8.
    #[error("failed to decode input as UTF-8: {0}")]
    Encoding(String),
}
