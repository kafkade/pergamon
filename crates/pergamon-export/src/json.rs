//! Versioned JSON export with stable export-specific DTOs.
//!
//! Exports the full pergamon data model as a JSON document with a
//! versioned schema. The output is decoupled from internal model
//! types via dedicated `*Export` structs so the public contract can
//! remain stable even as the internal model evolves.
//!
//! # Schema version
//!
//! The current schema version is **1**. The schema version is
//! incremented only for breaking changes.

use pergamon_core::model::{BookmarkMeta, ContentItem, FeedItemMeta, HighlightMeta, Note};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Current schema version for the JSON export format.
pub const SCHEMA_VERSION: u32 = 1;

// ======================================================================
// Export DTOs (decoupled from internal model)
// ======================================================================

/// Top-level JSON export document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonExport {
    /// Schema version for forward compatibility.
    pub schema_version: u32,
    /// pergamon version that produced this export.
    pub pergamon_version: String,
    /// ISO 8601 timestamp of the export.
    pub exported_at: String,
    /// Total number of items.
    pub item_count: usize,
    /// Exported items with all related data.
    pub items: Vec<JsonItemExport>,
}

/// A single content item with all related data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonItemExport {
    /// Stable unique identifier.
    pub id: String,
    /// Title.
    pub title: String,
    /// URL (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Author.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Content type discriminator.
    pub content_type: String,
    /// Lifecycle status.
    pub status: String,
    /// Full extracted text (opt-in, can be large).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_text: Option<String>,
    /// Short excerpt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    /// Publication date (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// Creation date (ISO 8601).
    pub created_at: String,
    /// Last update date (ISO 8601).
    pub updated_at: String,
    /// Tag names.
    pub tags: Vec<String>,
    /// Highlights on this item.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<JsonHighlightExport>,
    /// Notes on this item.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<JsonNoteExport>,
    /// Bookmark metadata (if content type is bookmark).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmark_meta: Option<JsonBookmarkMetaExport>,
    /// Feed item metadata (if content type is feed item).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed_item_meta: Option<JsonFeedItemMetaExport>,
}

/// A highlight in the JSON export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonHighlightExport {
    /// Highlight item ID.
    pub id: String,
    /// Source item ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_item_id: Option<String>,
    /// The highlighted text.
    pub quote_text: String,
    /// User note on the highlight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Start byte offset in source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_start: Option<i64>,
    /// End byte offset in source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_end: Option<i64>,
    /// Highlight color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Highlight creation date (ISO 8601).
    pub created_at: String,
}

/// A note in the JSON export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonNoteExport {
    /// Note ID.
    pub id: String,
    /// Note body text.
    pub body: String,
    /// Creation date (ISO 8601).
    pub created_at: String,
    /// Last update date (ISO 8601).
    pub updated_at: String,
}

/// Bookmark metadata in the JSON export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonBookmarkMetaExport {
    /// Original URL before canonicalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_url: Option<String>,
    /// Where the bookmark was saved from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_from: Option<String>,
    /// Thumbnail URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// Description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Site name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
}

/// Feed item metadata in the JSON export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonFeedItemMetaExport {
    /// Feed ID.
    pub feed_id: String,
    /// Feed-level GUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    /// Feed summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ======================================================================
// Configuration
// ======================================================================

/// Configuration for a JSON export.
#[derive(Debug, Clone)]
pub struct JsonExportConfig {
    /// Pretty-print the JSON output.
    pub pretty: bool,
    /// Include the full `content_text` field (can be very large).
    pub include_content_text: bool,
    /// The pergamon version string.
    pub pergamon_version: String,
}

impl Default for JsonExportConfig {
    fn default() -> Self {
        Self {
            pretty: true,
            include_content_text: false,
            pergamon_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

// ======================================================================
// Input bundle
// ======================================================================

/// Input data for a single item in the JSON export.
///
/// Callers build these from database queries and pass them to
/// [`build_json_export`].
#[derive(Debug, Clone)]
pub struct JsonExportItem {
    /// The content item.
    pub item: ContentItem,
    /// Tag names.
    pub tags: Vec<String>,
    /// Highlights with their metadata and item data.
    pub highlights: Vec<(ContentItem, HighlightMeta)>,
    /// Notes.
    pub notes: Vec<Note>,
    /// Bookmark metadata (if applicable).
    pub bookmark_meta: Option<BookmarkMeta>,
    /// Feed item metadata (if applicable).
    pub feed_item_meta: Option<FeedItemMeta>,
}

// ======================================================================
// Building
// ======================================================================

/// Build a JSON export document from input items.
///
/// This is a pure function — no I/O.
#[must_use]
pub fn build_json_export(config: &JsonExportConfig, items: &[JsonExportItem]) -> JsonExport {
    let now = format_rfc3339_now();

    let exported_items: Vec<JsonItemExport> = items
        .iter()
        .map(|ei| convert_item(ei, config.include_content_text))
        .collect();

    JsonExport {
        schema_version: SCHEMA_VERSION,
        pergamon_version: config.pergamon_version.clone(),
        exported_at: now,
        item_count: exported_items.len(),
        items: exported_items,
    }
}

/// Serialize a JSON export to a string.
///
/// # Errors
///
/// Returns a serialization error if the data cannot be encoded.
pub fn serialize_json_export(
    export: &JsonExport,
    pretty: bool,
) -> Result<String, serde_json::Error> {
    if pretty {
        serde_json::to_string_pretty(export)
    } else {
        serde_json::to_string(export)
    }
}

// ======================================================================
// Conversion
// ======================================================================

/// Convert an input item to an export DTO.
fn convert_item(ei: &JsonExportItem, include_content_text: bool) -> JsonItemExport {
    JsonItemExport {
        id: ei.item.id.to_string(),
        title: ei.item.title.clone(),
        url: ei.item.url.clone(),
        author: ei.item.author.clone(),
        content_type: ei.item.content_type.to_string(),
        status: ei.item.status.to_string(),
        content_text: if include_content_text {
            ei.item.content_text.clone()
        } else {
            None
        },
        excerpt: ei.item.excerpt.clone(),
        published_at: ei.item.published_at.map(format_datetime),
        created_at: format_datetime(ei.item.created_at),
        updated_at: format_datetime(ei.item.updated_at),
        tags: ei.tags.clone(),
        highlights: ei
            .highlights
            .iter()
            .map(|(item, meta)| convert_highlight(item, meta))
            .collect(),
        notes: ei.notes.iter().map(convert_note).collect(),
        bookmark_meta: ei.bookmark_meta.as_ref().map(convert_bookmark_meta),
        feed_item_meta: ei.feed_item_meta.as_ref().map(convert_feed_item_meta),
    }
}

fn convert_highlight(item: &ContentItem, meta: &HighlightMeta) -> JsonHighlightExport {
    JsonHighlightExport {
        id: item.id.to_string(),
        source_item_id: meta.source_item_id.map(|u| u.to_string()),
        quote_text: meta.quote_text.clone(),
        note: meta.note.clone(),
        position_start: meta.position_start,
        position_end: meta.position_end,
        color: meta.color.clone(),
        created_at: format_datetime(item.created_at),
    }
}

fn convert_note(note: &Note) -> JsonNoteExport {
    JsonNoteExport {
        id: note.id.to_string(),
        body: note.body.clone(),
        created_at: format_datetime(note.created_at),
        updated_at: format_datetime(note.updated_at),
    }
}

fn convert_bookmark_meta(meta: &BookmarkMeta) -> JsonBookmarkMetaExport {
    JsonBookmarkMetaExport {
        original_url: meta.original_url.clone(),
        saved_from: meta.saved_from.clone(),
        thumbnail_url: meta.thumbnail_url.clone(),
        description: meta.description.clone(),
        site_name: meta.site_name.clone(),
    }
}

fn convert_feed_item_meta(meta: &FeedItemMeta) -> JsonFeedItemMetaExport {
    JsonFeedItemMetaExport {
        feed_id: meta.feed_id.to_string(),
        guid: meta.guid.clone(),
        summary: meta.summary.clone(),
    }
}

// ======================================================================
// Helpers
// ======================================================================

/// Format an `OffsetDateTime` as ISO 8601 / RFC 3339.
fn format_datetime(dt: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    )
}

/// Format the current time as RFC 3339.
fn format_rfc3339_now() -> String {
    format_datetime(OffsetDateTime::now_utc())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use pergamon_core::content_type::ContentType;
    use pergamon_core::status::DocumentStatus;
    use uuid::Uuid;

    fn make_item(title: &str) -> ContentItem {
        ContentItem {
            id: Uuid::new_v4(),
            url: Some("https://example.com".to_owned()),
            title: title.to_owned(),
            author: Some("Test Author".to_owned()),
            content_type: ContentType::Article,
            status: DocumentStatus::Archived,
            content_text: Some("Full article text.".to_owned()),
            excerpt: Some("A short excerpt.".to_owned()),
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn basic_json_export() {
        let config = JsonExportConfig::default();
        let items = vec![JsonExportItem {
            item: make_item("Test Article"),
            tags: vec!["rust".to_owned()],
            highlights: vec![],
            notes: vec![],
            bookmark_meta: None,
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);

        assert_eq!(export.schema_version, SCHEMA_VERSION);
        assert_eq!(export.item_count, 1);
        assert_eq!(export.items[0].title, "Test Article");
        assert_eq!(export.items[0].content_type, "article");
        assert_eq!(export.items[0].tags, vec!["rust"]);
    }

    #[test]
    fn content_text_excluded_by_default() {
        let config = JsonExportConfig::default();
        let items = vec![JsonExportItem {
            item: make_item("Article"),
            tags: vec![],
            highlights: vec![],
            notes: vec![],
            bookmark_meta: None,
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);
        assert!(export.items[0].content_text.is_none());
    }

    #[test]
    fn content_text_included_when_enabled() {
        let config = JsonExportConfig {
            include_content_text: true,
            ..JsonExportConfig::default()
        };
        let items = vec![JsonExportItem {
            item: make_item("Article"),
            tags: vec![],
            highlights: vec![],
            notes: vec![],
            bookmark_meta: None,
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);
        assert_eq!(
            export.items[0].content_text.as_deref(),
            Some("Full article text.")
        );
    }

    #[test]
    fn highlights_included() {
        let source = make_item("Source");
        let source_id = source.id;

        let hl_item = ContentItem {
            id: Uuid::new_v4(),
            content_type: ContentType::Highlight,
            status: DocumentStatus::Inbox,
            title: String::new(),
            url: None,
            author: None,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        let hl_meta = HighlightMeta {
            content_item_id: hl_item.id,
            source_item_id: Some(source_id),
            quote_text: "Highlighted text.".to_owned(),
            note: Some("A note.".to_owned()),
            position_start: Some(10),
            position_end: Some(28),
            color: Some("yellow".to_owned()),
        };

        let config = JsonExportConfig::default();
        let items = vec![JsonExportItem {
            item: source,
            tags: vec![],
            highlights: vec![(hl_item, hl_meta)],
            notes: vec![],
            bookmark_meta: None,
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);
        assert_eq!(export.items[0].highlights.len(), 1);

        let hl = &export.items[0].highlights[0];
        assert_eq!(hl.quote_text, "Highlighted text.");
        assert_eq!(hl.note.as_deref(), Some("A note."));
        assert_eq!(hl.position_start, Some(10));
        assert_eq!(hl.color.as_deref(), Some("yellow"));
    }

    #[test]
    fn notes_included() {
        let item = make_item("Noted");
        let item_id = item.id;

        let config = JsonExportConfig::default();
        let items = vec![JsonExportItem {
            item,
            tags: vec![],
            highlights: vec![],
            notes: vec![Note {
                id: Uuid::new_v4(),
                content_item_id: item_id,
                body: "My note.".to_owned(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            }],
            bookmark_meta: None,
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);
        assert_eq!(export.items[0].notes.len(), 1);
        assert_eq!(export.items[0].notes[0].body, "My note.");
    }

    #[test]
    fn bookmark_meta_included() {
        let mut item = make_item("Bookmark");
        item.content_type = ContentType::Bookmark;

        let config = JsonExportConfig::default();
        let items = vec![JsonExportItem {
            item: item.clone(),
            tags: vec![],
            highlights: vec![],
            notes: vec![],
            bookmark_meta: Some(BookmarkMeta {
                content_item_id: item.id,
                original_url: Some("https://example.com/original".to_owned()),
                saved_from: Some("browser".to_owned()),
                thumbnail_url: None,
                description: Some("A useful link.".to_owned()),
                site_name: Some("Example".to_owned()),
                favicon_url: None,
            }),
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);
        let bm = export.items[0].bookmark_meta.as_ref().unwrap();
        assert_eq!(bm.description.as_deref(), Some("A useful link."));
        assert_eq!(bm.site_name.as_deref(), Some("Example"));
    }

    #[test]
    fn serialize_pretty_and_compact() {
        let config = JsonExportConfig::default();
        let items = vec![JsonExportItem {
            item: make_item("Serialize Test"),
            tags: vec![],
            highlights: vec![],
            notes: vec![],
            bookmark_meta: None,
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);

        let pretty = serialize_json_export(&export, true).unwrap();
        assert!(pretty.contains('\n'));
        assert!(pretty.contains("\"schema_version\": 1"));

        let compact = serialize_json_export(&export, false).unwrap();
        assert!(!compact.contains('\n'));
        assert!(compact.contains("\"schema_version\":1"));
    }

    #[test]
    fn optional_fields_skipped_when_none() {
        let config = JsonExportConfig::default();
        let mut item = make_item("Minimal");
        item.url = None;
        item.author = None;
        item.excerpt = None;
        item.content_text = None;
        item.published_at = None;

        let items = vec![JsonExportItem {
            item,
            tags: vec![],
            highlights: vec![],
            notes: vec![],
            bookmark_meta: None,
            feed_item_meta: None,
        }];

        let export = build_json_export(&config, &items);
        let json = serialize_json_export(&export, true).unwrap();

        // Optional fields should not appear in output.
        assert!(!json.contains("\"url\""));
        assert!(!json.contains("\"author\""));
        assert!(!json.contains("\"excerpt\""));
        assert!(!json.contains("\"published_at\""));
        assert!(!json.contains("\"content_text\""));
        assert!(!json.contains("\"bookmark_meta\""));
        assert!(!json.contains("\"feed_item_meta\""));
        assert!(!json.contains("\"highlights\""));
        assert!(!json.contains("\"notes\""));
    }

    #[test]
    fn feed_item_meta_included() {
        let item = make_item("Feed Item");
        let feed_id = Uuid::new_v4();

        let config = JsonExportConfig::default();
        let items = vec![JsonExportItem {
            item: item.clone(),
            tags: vec![],
            highlights: vec![],
            notes: vec![],
            bookmark_meta: None,
            feed_item_meta: Some(FeedItemMeta {
                content_item_id: item.id,
                feed_id,
                guid: Some("urn:uuid:abc123".to_owned()),
                summary: Some("A feed summary.".to_owned()),
            }),
        }];

        let export = build_json_export(&config, &items);
        let fi = export.items[0].feed_item_meta.as_ref().unwrap();
        assert_eq!(fi.feed_id, feed_id.to_string());
        assert_eq!(fi.guid.as_deref(), Some("urn:uuid:abc123"));
    }
}
