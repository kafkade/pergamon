//! Domain model types for the unified content system.
//!
//! These structs represent the canonical entities in pergamon's data model.
//! They are pure data — no I/O, no database coupling. The storage layer
//! maps these to/from `SQLite` rows.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::content_type::ContentType;
use crate::status::DocumentStatus;

/// A unified content item — the canonical unit of saved content.
///
/// Every saved item (feed entry, article, bookmark, highlight, PDF, podcast
/// episode) shares this common shape. Type-specific metadata lives in
/// extension structs ([`FeedItemMeta`], [`BookmarkMeta`], [`HighlightMeta`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentItem {
    /// Stable unique identifier.
    pub id: Uuid,
    /// URL of the content (if applicable).
    pub url: Option<String>,
    /// Title of the content item.
    pub title: String,
    /// Author or creator.
    pub author: Option<String>,
    /// Discriminator for the content type.
    pub content_type: ContentType,
    /// Lifecycle status in the triage workflow.
    pub status: DocumentStatus,
    /// Normalized extracted text (for FTS and reading).
    pub content_text: Option<String>,
    /// Short excerpt or summary.
    pub excerpt: Option<String>,
    /// Publication timestamp (if known).
    pub published_at: Option<OffsetDateTime>,
    /// When this item was created in pergamon.
    pub created_at: OffsetDateTime,
    /// When this item was last updated.
    pub updated_at: OffsetDateTime,
    /// When this item was read (status transitioned to archived).
    pub read_at: Option<OffsetDateTime>,
}

/// An RSS/Atom feed subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    /// Stable unique identifier.
    pub id: Uuid,
    /// Display title of the feed.
    pub title: String,
    /// Feed URL (RSS/Atom endpoint).
    pub url: String,
    /// Website URL of the feed source.
    pub site_url: Option<String>,
    /// Feed description.
    pub description: Option<String>,
    /// HTTP `ETag` header from the last fetch (for conditional GET).
    pub etag: Option<String>,
    /// HTTP `Last-Modified` header from the last fetch (for conditional GET).
    pub last_modified_header: Option<String>,
    /// Number of consecutive fetch errors.
    pub error_count: i32,
    /// Description of the last fetch error.
    pub last_error: Option<String>,
    /// When the feed was last successfully fetched (content changed or 304).
    pub last_fetched_at: Option<OffsetDateTime>,
    /// Folder this feed belongs to (for OPML categories).
    pub folder_id: Option<Uuid>,
    /// When this feed was added to pergamon.
    pub created_at: OffsetDateTime,
    /// When this feed record was last updated.
    pub updated_at: OffsetDateTime,
}

/// A folder for organising feed subscriptions (OPML categories).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedFolder {
    /// Stable unique identifier.
    pub id: Uuid,
    /// Display name of the folder.
    pub name: String,
    /// Parent folder ID (for nested hierarchies).
    pub parent_id: Option<Uuid>,
    /// When this folder was created.
    pub created_at: OffsetDateTime,
    /// When this folder was last updated.
    pub updated_at: OffsetDateTime,
}

/// Extension metadata for feed items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedItemMeta {
    /// ID of the associated content item.
    pub content_item_id: Uuid,
    /// ID of the source feed.
    pub feed_id: Uuid,
    /// Feed-level GUID or entry ID.
    pub guid: Option<String>,
    /// Feed-provided summary or description.
    pub summary: Option<String>,
}

/// Extension metadata for bookmarks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkMeta {
    /// ID of the associated content item.
    pub content_item_id: Uuid,
    /// The original URL as captured (before normalization).
    pub original_url: Option<String>,
    /// Where the bookmark was saved from (e.g., "browser", "share sheet").
    pub saved_from: Option<String>,
    /// Thumbnail / preview image URL (from OG or Twitter Card).
    pub thumbnail_url: Option<String>,
    /// User-provided or auto-generated description.
    pub description: Option<String>,
    /// Site name (from `og:site_name` or similar).
    pub site_name: Option<String>,
    /// Favicon URL.
    pub favicon_url: Option<String>,
}

/// Link health check result for a content item.
///
/// Tracks the HTTP status, final destination (after redirects), redirect
/// count, and any connection errors for a URL. Used by `pergamon doctor links`
/// to detect dead links, redirects, and domain changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkHealth {
    /// ID of the associated content item.
    pub content_item_id: Uuid,
    /// HTTP status code from the final response (`None` if the request failed
    /// before receiving a response — e.g., DNS failure, timeout).
    pub http_status: Option<i32>,
    /// The URL after following all redirects (`None` if no redirect occurred).
    pub final_url: Option<String>,
    /// Number of HTTP redirects followed to reach the final URL.
    pub redirect_count: i32,
    /// When this health check was performed.
    pub last_checked_at: OffsetDateTime,
    /// Error description when the request failed (timeout, DNS, TLS, etc.).
    pub error_message: Option<String>,
}

/// Extension metadata for highlights / annotations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightMeta {
    /// ID of the associated content item.
    pub content_item_id: Uuid,
    /// ID of the source document this highlight was taken from.
    pub source_item_id: Option<Uuid>,
    /// The highlighted text.
    pub quote_text: String,
    /// User note attached to the highlight.
    pub note: Option<String>,
    /// Start offset of the highlight in the source text.
    pub position_start: Option<i64>,
    /// End offset of the highlight in the source text.
    pub position_end: Option<i64>,
    /// Highlight color label.
    pub color: Option<String>,
}

/// A free-form note attached to any content item.
///
/// Notes are standalone annotations that can be attached to articles,
/// bookmarks, highlights, or any other content item. Unlike the inline
/// `note` field on [`HighlightMeta`], these are first-class entities
/// with their own identity and lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Stable unique identifier.
    pub id: Uuid,
    /// ID of the content item this note is attached to.
    pub content_item_id: Uuid,
    /// Free-form text body of the note.
    pub body: String,
    /// When this note was created.
    pub created_at: OffsetDateTime,
    /// When this note was last updated.
    pub updated_at: OffsetDateTime,
}

/// A user-defined tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    /// Stable unique identifier.
    pub id: Uuid,
    /// Tag name (case-normalized).
    pub name: String,
    /// When this tag was created.
    pub created_at: OffsetDateTime,
}

/// A hierarchical collection (folder).
///
/// Collections can be either manual (items added/removed explicitly) or
/// *smart* (membership computed dynamically from a filter query).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    /// Stable unique identifier.
    pub id: Uuid,
    /// Display name.
    pub name: String,
    /// Parent collection ID (for nesting).
    pub parent_id: Option<Uuid>,
    /// Sort order within the parent.
    pub sort_order: i32,
    /// Whether this is a smart (auto-populated) collection.
    pub is_smart: bool,
    /// The filter query string for smart collections (DSL syntax).
    pub filter_query: Option<String>,
    /// When this collection was created.
    pub created_at: OffsetDateTime,
    /// When this collection was last updated.
    pub updated_at: OffsetDateTime,
}

/// A search result from the FTS5 index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    /// ID of the matching content item.
    pub content_item_id: Uuid,
    /// BM25 relevance rank (lower is more relevant).
    pub rank: f64,
}

/// A rich search hit: full content item plus relevance and snippet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// The matching content item.
    pub item: ContentItem,
    /// BM25 relevance rank (lower is more relevant).
    pub rank: f64,
    /// Snippet with match context from the best-matching FTS column.
    pub snippet: Option<String>,
}

/// A spaced-repetition review card linked to a highlight.
///
/// Each highlight can optionally have one review card that tracks its
/// FSRS scheduling state. Created via `review enable`, removed via
/// `review disable`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewCard {
    /// Stable unique identifier.
    pub id: Uuid,
    /// ID of the highlight's `content_item` (FK → `highlight_meta`).
    pub content_item_id: Uuid,
    /// Current card state in the FSRS lifecycle.
    pub state: crate::fsrs::CardState,
    /// Current stability (days until 90% recall). `None` for new cards.
    pub stability: Option<f64>,
    /// Current difficulty (1.0–10.0). `None` for new cards.
    pub difficulty: Option<f64>,
    /// When the next review is due.
    pub due_at: OffsetDateTime,
    /// When the card was last reviewed. `None` if never reviewed.
    pub last_reviewed_at: Option<OffsetDateTime>,
    /// Total number of reviews performed.
    pub review_count: i32,
    /// Number of lapses (Again ratings).
    pub lapse_count: i32,
    /// Last scheduled interval in days. `None` for new cards.
    pub scheduled_days: Option<f64>,
    /// When this card was created.
    pub created_at: OffsetDateTime,
    /// When this card was last updated.
    pub updated_at: OffsetDateTime,
}

/// A log entry recording a single review event.
///
/// Captures both the before and after state of a review for analytics
/// and potential future algorithm optimization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewLog {
    /// Stable unique identifier.
    pub id: Uuid,
    /// ID of the review card (FK → `review_cards`).
    pub card_id: Uuid,
    /// Rating given by the user.
    pub rating: crate::fsrs::Rating,
    /// Card state before this review.
    pub state_before: crate::fsrs::CardState,
    /// Stability before this review.
    pub stability_before: Option<f64>,
    /// Difficulty before this review.
    pub difficulty_before: Option<f64>,
    /// Card state after this review.
    pub state_after: crate::fsrs::CardState,
    /// Stability after this review.
    pub stability_after: f64,
    /// Difficulty after this review.
    pub difficulty_after: f64,
    /// Days elapsed since the last review (or card creation).
    pub elapsed_days: f64,
    /// Scheduled interval in days after this review.
    pub scheduled_days: f64,
    /// When this review was performed.
    pub reviewed_at: OffsetDateTime,
}

/// Aggregated review statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewStats {
    /// Total number of review cards.
    pub total_cards: i64,
    /// Number of cards currently due.
    pub due_count: i64,
    /// Total number of reviews ever performed.
    pub total_reviews: i64,
    /// Number of successful reviews (rating ≥ Hard).
    pub success_count: i64,
    /// Observed retention rate (`success_count` / `total_reviews`).
    pub observed_retention: f64,
    /// Number of new cards (never reviewed).
    pub new_count: i64,
    /// Number of cards in learning state.
    pub learning_count: i64,
    /// Number of cards in review state.
    pub review_count: i64,
    /// Number of cards in relearning state.
    pub relearning_count: i64,
    /// Number of reviews completed today (UTC).
    pub reviews_today: i64,
    /// Current consecutive-day review streak.
    pub current_streak: i64,
    /// Longest consecutive-day review streak ever achieved.
    pub longest_streak: i64,
}

/// Review card count grouped by provenance (source origin).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBreakdown {
    /// Label for the source origin (e.g. "Kindle", "Readwise", "Feed", "Manual").
    pub origin: String,
    /// Number of review cards from this origin.
    pub count: i64,
}

/// Daily review activity for trend charts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyReviewSummary {
    /// Date in YYYY-MM-DD format (UTC).
    pub date: String,
    /// Total reviews performed that day.
    pub reviews: i64,
    /// Reviews rated Hard or above (successful).
    pub successes: i64,
}

/// Composite report bundling all review statistics.
///
/// Designed for JSON serialisation and shared between CLI and TUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewStatsReport {
    /// Core review statistics with streaks.
    pub stats: ReviewStats,
    /// Review cards grouped by source provenance.
    pub source_breakdown: Vec<SourceBreakdown>,
    /// Daily review counts for the last 30 days.
    pub daily_history: Vec<DailyReviewSummary>,
    /// Weekly review counts (last 12 weeks).
    pub weekly_history: Vec<WeeklyReviewSummary>,
}

/// Weekly review activity summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeeklyReviewSummary {
    /// ISO week label (e.g. "2026-W22").
    pub week: String,
    /// Total reviews performed that week.
    pub reviews: i64,
    /// Reviews rated Hard or above (successful).
    pub successes: i64,
}

// ======================================================================
// Usage & reading analytics
// ======================================================================

/// Composite usage statistics report.
///
/// Bundles all reading and content analytics into a single struct for
/// JSON serialisation and shared use between CLI and TUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageStatsReport {
    /// High-level overview counts and rates.
    pub overview: UsageOverview,
    /// Reading activity broken down by day, week, and month.
    pub reading_activity: ReadingActivity,
    /// Top content sources ranked by read count.
    pub top_sources: Vec<SourceRanking>,
    /// Tag distribution: most-used tags with counts.
    pub tag_distribution: Vec<TagCount>,
    /// Tag usage over time (monthly buckets).
    pub tag_trends: Vec<TagTrendPoint>,
}

/// High-level usage overview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageOverview {
    /// Total content items in the library.
    pub total_items: u64,
    /// Items currently in inbox.
    pub inbox_count: u64,
    /// Items marked as archived (completed reading).
    pub archived_count: u64,
    /// Total highlights created.
    pub total_highlights: u64,
    /// Total feed subscriptions.
    pub total_feeds: u64,
    /// Items saved today.
    pub items_saved_today: u64,
    /// Items saved this week.
    pub items_saved_this_week: u64,
    /// Items saved this month.
    pub items_saved_this_month: u64,
    /// Average items saved per day over the last 30 days.
    pub saves_per_day_30d: f64,
    /// Highlights per content item (highlight rate).
    pub highlight_rate: f64,
    /// Estimated total reading minutes (archived items only).
    pub total_reading_minutes: u64,
    /// Current consecutive-day reading streak.
    pub reading_streak_days: i64,
    /// Longest consecutive-day reading streak ever.
    pub longest_reading_streak: i64,
}

/// Reading activity broken down by time period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadingActivity {
    /// Daily activity for the last 30 days (zero-filled).
    pub daily: Vec<DailyUsageSummary>,
    /// Weekly activity for the last 12 weeks (zero-filled).
    pub weekly: Vec<WeeklyUsageSummary>,
    /// Monthly activity for the last 12 months (zero-filled).
    pub monthly: Vec<MonthlyUsageSummary>,
}

/// Daily usage and reading activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyUsageSummary {
    /// Date in YYYY-MM-DD format.
    pub date: String,
    /// Items saved (created) that day.
    pub items_saved: i64,
    /// Items read (archived) that day.
    pub items_read: i64,
    /// Estimated reading time in minutes.
    pub reading_minutes: i64,
}

/// Weekly usage and reading activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeeklyUsageSummary {
    /// ISO week label (e.g. "2026-W22").
    pub week: String,
    /// Items saved that week.
    pub items_saved: i64,
    /// Items read that week.
    pub items_read: i64,
    /// Estimated reading time in minutes.
    pub reading_minutes: i64,
}

/// Monthly usage and reading activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonthlyUsageSummary {
    /// Month label (e.g. "2026-05").
    pub month: String,
    /// Items saved that month.
    pub items_saved: i64,
    /// Items read that month.
    pub items_read: i64,
    /// Estimated reading time in minutes.
    pub reading_minutes: i64,
}

/// A content source ranked by read count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRanking {
    /// Source name (feed title or domain).
    pub source_name: String,
    /// Number of items read from this source.
    pub items_read: i64,
    /// Total items from this source.
    pub total_items: i64,
}

/// Tag usage count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagCount {
    /// Tag name.
    pub tag_name: String,
    /// Number of items with this tag.
    pub count: i64,
}

/// Tag usage at a point in time (for trend charts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagTrendPoint {
    /// Month label (e.g. "2026-05").
    pub month: String,
    /// Tag name.
    pub tag_name: String,
    /// Number of items tagged in this month.
    pub count: i64,
}
