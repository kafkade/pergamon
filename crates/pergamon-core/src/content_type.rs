//! Content type taxonomy for the unified data model.
//!
//! Every saved item in pergamon is a `content_item` distinguished by its
//! [`ContentType`]. See `docs/adr/002-content-type-taxonomy-and-unified-data-model.md`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Discriminator for the unified `content_items` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    /// An item ingested from an RSS/Atom feed.
    FeedItem,
    /// A web article captured for reading.
    Article,
    /// A saved bookmark (may optionally have extracted content).
    Bookmark,
    /// A user highlight or annotation.
    Highlight,
    /// A PDF document.
    Pdf,
    /// A podcast episode.
    PodcastEpisode,
}

impl ContentType {
    /// Returns the canonical string representation used in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FeedItem => "feed_item",
            Self::Article => "article",
            Self::Bookmark => "bookmark",
            Self::Highlight => "highlight",
            Self::Pdf => "pdf",
            Self::PodcastEpisode => "podcast_episode",
        }
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ContentType {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "feed_item" => Ok(Self::FeedItem),
            "article" => Ok(Self::Article),
            "bookmark" => Ok(Self::Bookmark),
            "highlight" => Ok(Self::Highlight),
            "pdf" => Ok(Self::Pdf),
            "podcast_episode" => Ok(Self::PodcastEpisode),
            other => Err(CoreError::UnknownContentType(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_variants() {
        let variants = [
            ContentType::FeedItem,
            ContentType::Article,
            ContentType::Bookmark,
            ContentType::Highlight,
            ContentType::Pdf,
            ContentType::PodcastEpisode,
        ];
        for ct in variants {
            let s = ct.to_string();
            let parsed: ContentType = s.parse().unwrap_or_else(|e| {
                let _ = e;
                unreachable!("failed to parse ContentType from {s:?}")
            });
            assert_eq!(ct, parsed);
        }
    }

    #[test]
    fn invalid_content_type() {
        let result = "unknown".parse::<ContentType>();
        assert!(result.is_err());
    }
}
