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
