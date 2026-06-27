//! Diagnostics domain types backing the admin diagnostics view.
//!
//! These are pure data structures (and small classification helpers) with no
//! I/O. The storage layer populates them from `SQLite` rows; the server renders
//! them in the admin diagnostics pages.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

// ======================================================================
// Import history
// ======================================================================

/// The external source an import run came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSource {
    /// OPML feed-subscription import.
    Opml,
    /// Raindrop.io CSV bookmark import.
    Raindrop,
    /// Pocket HTML bookmark import.
    Pocket,
    /// Kindle `My Clippings.txt` highlight import.
    Kindle,
    /// Readwise CSV highlight import.
    Readwise,
    /// Full backup restore.
    Backup,
}

impl ImportSource {
    /// Stable lowercase identifier used in the database and APIs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opml => "opml",
            Self::Raindrop => "raindrop",
            Self::Pocket => "pocket",
            Self::Kindle => "kindle",
            Self::Readwise => "readwise",
            Self::Backup => "backup",
        }
    }

    /// Parse a stable identifier back into a variant.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "opml" => Some(Self::Opml),
            "raindrop" => Some(Self::Raindrop),
            "pocket" => Some(Self::Pocket),
            "kindle" => Some(Self::Kindle),
            "readwise" => Some(Self::Readwise),
            "backup" => Some(Self::Backup),
            _ => None,
        }
    }

    /// Human-friendly label for display.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Opml => "OPML",
            Self::Raindrop => "Raindrop.io",
            Self::Pocket => "Pocket",
            Self::Kindle => "Kindle",
            Self::Readwise => "Readwise",
            Self::Backup => "Backup",
        }
    }
}

/// One recorded import run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportLogEntry {
    /// Stable unique identifier.
    pub id: Uuid,
    /// Which importer produced this run.
    pub source: ImportSource,
    /// Name of the imported file, if known.
    pub file_name: Option<String>,
    /// Number of items newly added.
    pub items_added: i64,
    /// Number of items that already existed (idempotent re-import).
    pub items_existing: i64,
    /// Number of items skipped (malformed, filtered, etc.).
    pub items_skipped: i64,
    /// Number of errors encountered.
    pub errors: i64,
    /// Optional human-readable error detail.
    pub error_detail: Option<String>,
    /// Whether this run was a dry run (no writes).
    pub dry_run: bool,
    /// When the run completed.
    pub created_at: OffsetDateTime,
}

// ======================================================================
// Extraction events
// ======================================================================

/// The code path that triggered an extraction attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionSource {
    /// CLI `save` command.
    Save,
    /// Feed sync ingestion.
    FeedSync,
    /// Web bookmark add.
    Bookmark,
}

impl ExtractionSource {
    /// Stable lowercase identifier used in the database and APIs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::FeedSync => "feed_sync",
            Self::Bookmark => "bookmark",
        }
    }

    /// Parse a stable identifier back into a variant.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "save" => Some(Self::Save),
            "feed_sync" => Some(Self::FeedSync),
            "bookmark" => Some(Self::Bookmark),
            _ => None,
        }
    }

    /// Human-friendly label for display.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Save => "Save",
            Self::FeedSync => "Feed sync",
            Self::Bookmark => "Bookmark",
        }
    }
}

/// One recorded content-extraction attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionEvent {
    /// Stable unique identifier.
    pub id: Uuid,
    /// The content item the extraction produced (if it succeeded and was stored).
    pub content_item_id: Option<Uuid>,
    /// The URL that was extracted.
    pub url: Option<String>,
    /// Which code path triggered the extraction.
    pub source: ExtractionSource,
    /// Whether extraction succeeded.
    pub success: bool,
    /// The extractor used (e.g. `readability`, `pdf`, `metadata`).
    pub extractor: Option<String>,
    /// Failure detail when `success` is false.
    pub error_message: Option<String>,
    /// When the attempt happened.
    pub created_at: OffsetDateTime,
}

/// Aggregate extraction statistics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExtractionStats {
    /// Total extraction attempts in the window.
    pub total: i64,
    /// Number that succeeded.
    pub succeeded: i64,
    /// Number that failed.
    pub failed: i64,
    /// Success rate as a percentage (0.0–100.0).
    pub success_rate: f64,
}

impl ExtractionStats {
    /// Build aggregate stats from success/failure counts, computing the rate.
    #[must_use]
    pub fn new(succeeded: i64, failed: i64) -> Self {
        let total = succeeded + failed;
        let success_rate = if total > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                (succeeded as f64 / total as f64) * 100.0
            }
        } else {
            0.0
        };
        Self {
            total,
            succeeded,
            failed,
            success_rate,
        }
    }
}

// ======================================================================
// Feed health
// ======================================================================

/// Health classification for a feed subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedHealthStatus {
    /// No recent fetch errors.
    Healthy,
    /// A small number of consecutive errors.
    Warning,
    /// Repeated consecutive errors.
    Error,
}

impl FeedHealthStatus {
    /// Classify a feed from its consecutive error count.
    ///
    /// `0` errors is healthy, `1`–`2` is a warning, `3`+ is an error.
    #[must_use]
    pub const fn from_error_count(error_count: i32) -> Self {
        match error_count {
            0 => Self::Healthy,
            1 | 2 => Self::Warning,
            _ => Self::Error,
        }
    }

    /// Stable lowercase identifier for CSS classes / display.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A feed's health summary row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedHealthRow {
    /// Feed identifier.
    pub feed_id: Uuid,
    /// Feed title.
    pub title: String,
    /// Feed URL.
    pub url: String,
    /// Health classification.
    pub status: FeedHealthStatus,
    /// Consecutive fetch errors.
    pub error_count: i32,
    /// Last fetch error message.
    pub last_error: Option<String>,
    /// When the feed was last successfully fetched.
    pub last_fetched_at: Option<OffsetDateTime>,
    /// Whether the feed has not updated within the staleness threshold.
    pub is_stale: bool,
}

// ======================================================================
// System statistics
// ======================================================================

/// Count of content items by content type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentTypeCount {
    /// Content type identifier.
    pub content_type: String,
    /// Number of items of this type.
    pub count: i64,
}

/// Count of content items by lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCount {
    /// Status identifier.
    pub status: String,
    /// Number of items in this status.
    pub count: i64,
}

/// High-level system statistics for the admin overview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemStats {
    /// Total content items.
    pub total_items: i64,
    /// Total feed subscriptions.
    pub total_feeds: i64,
    /// Total tags.
    pub total_tags: i64,
    /// Total collections.
    pub total_collections: i64,
    /// Total highlights.
    pub total_highlights: i64,
    /// Total notes.
    pub total_notes: i64,
    /// Total review cards.
    pub total_review_cards: i64,
    /// Approximate on-disk database size in bytes.
    pub db_size_bytes: i64,
    /// Whether the FTS index passed an integrity check.
    pub fts_ok: bool,
    /// Item distribution by content type.
    pub content_types: Vec<ContentTypeCount>,
    /// Item distribution by status.
    pub statuses: Vec<StatusCount>,
}

// ======================================================================
// Link health
// ======================================================================

/// A broken link surfaced by link-health checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokenLinkRow {
    /// The content item the link belongs to.
    pub content_item_id: Uuid,
    /// The item title.
    pub title: String,
    /// The item URL.
    pub url: Option<String>,
    /// The HTTP status recorded at the last check.
    pub http_status: Option<i64>,
    /// Error message recorded at the last check.
    pub error_message: Option<String>,
    /// When the link was last checked.
    pub last_checked_at: OffsetDateTime,
}

// ======================================================================
// Content-rules monitor
// ======================================================================

/// A content rule with its current match statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleMonitorRow {
    /// Rule identifier.
    pub rule_id: Uuid,
    /// Rule name.
    pub name: String,
    /// Whether the rule is enabled.
    pub enabled: bool,
    /// Rule priority (higher runs first).
    pub priority: i64,
    /// The filter query used to match items.
    pub filter_query: String,
    /// Number of items currently matching the filter.
    pub match_count: i64,
    /// Short human-readable summary of the rule's actions.
    pub action_summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_stats_computes_rate() {
        let s = ExtractionStats::new(3, 1);
        assert_eq!(s.total, 4);
        assert_eq!(s.succeeded, 3);
        assert_eq!(s.failed, 1);
        assert!((s.success_rate - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn extraction_stats_zero_total_is_zero_rate() {
        let s = ExtractionStats::new(0, 0);
        assert_eq!(s.total, 0);
        assert!((s.success_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn feed_health_classification() {
        assert_eq!(
            FeedHealthStatus::from_error_count(0),
            FeedHealthStatus::Healthy
        );
        assert_eq!(
            FeedHealthStatus::from_error_count(1),
            FeedHealthStatus::Warning
        );
        assert_eq!(
            FeedHealthStatus::from_error_count(2),
            FeedHealthStatus::Warning
        );
        assert_eq!(
            FeedHealthStatus::from_error_count(3),
            FeedHealthStatus::Error
        );
        assert_eq!(
            FeedHealthStatus::from_error_count(99),
            FeedHealthStatus::Error
        );
    }

    #[test]
    fn source_roundtrips() {
        for src in [
            ImportSource::Opml,
            ImportSource::Raindrop,
            ImportSource::Pocket,
            ImportSource::Kindle,
            ImportSource::Readwise,
            ImportSource::Backup,
        ] {
            assert_eq!(ImportSource::from_db_str(src.as_str()), Some(src));
        }
        for src in [
            ExtractionSource::Save,
            ExtractionSource::FeedSync,
            ExtractionSource::Bookmark,
        ] {
            assert_eq!(ExtractionSource::from_db_str(src.as_str()), Some(src));
        }
    }
}
