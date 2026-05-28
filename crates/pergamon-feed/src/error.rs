//! Errors for the feed parsing crate.

/// Errors that can occur during feed parsing.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    /// The feed could not be parsed by `feed-rs`.
    #[error("failed to parse feed: {0}")]
    Parse(String),

    /// The OPML document could not be parsed or generated.
    #[error("OPML error: {0}")]
    Opml(String),
}
