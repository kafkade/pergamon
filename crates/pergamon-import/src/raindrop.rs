//! Raindrop.io CSV export parser.
//!
//! Raindrop exports bookmarks as CSV with these columns:
//! `id,title,note,excerpt,url,folder,tags,created,cover,highlights`

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Iso8601;

use crate::error::ImportError;

/// A single item parsed from a Raindrop.io CSV export.
#[derive(Debug, Clone)]
pub struct RaindropItem {
    /// Raindrop's internal numeric ID.
    pub id: String,
    /// Bookmark title.
    pub title: String,
    /// User-written note.
    pub note: Option<String>,
    /// Auto-generated excerpt.
    pub excerpt: Option<String>,
    /// The bookmarked URL.
    pub url: String,
    /// Folder/collection name (may contain " / " for nesting).
    pub folder: Option<String>,
    /// Tags associated with the bookmark.
    pub tags: Vec<String>,
    /// When the bookmark was created.
    pub created: Option<OffsetDateTime>,
    /// Cover image URL.
    pub cover: Option<String>,
    /// Highlight snippets.
    pub highlights: Vec<String>,
}

/// Raw CSV row from the Raindrop export.
#[derive(Debug, Deserialize)]
struct RaindropRow {
    id: String,
    title: String,
    note: String,
    excerpt: String,
    url: String,
    folder: String,
    tags: String,
    created: String,
    cover: String,
    highlights: String,
}

/// Parse a Raindrop.io CSV export from raw bytes.
///
/// Expects the standard Raindrop CSV header row followed by data rows.
///
/// # Errors
///
/// Returns `ImportError::Csv` if the CSV is malformed or columns are missing,
/// or `ImportError::Timestamp` if a `created` value is not valid ISO 8601.
pub fn parse_raindrop_csv(bytes: &[u8]) -> Result<Vec<RaindropItem>, ImportError> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(bytes);

    let mut items = Vec::new();
    for result in reader.deserialize() {
        let row: RaindropRow = result?;
        items.push(row_to_item(row)?);
    }
    Ok(items)
}

/// Convert a raw CSV row into a `RaindropItem`.
fn row_to_item(row: RaindropRow) -> Result<RaindropItem, ImportError> {
    let note = if row.note.is_empty() {
        None
    } else {
        Some(row.note)
    };
    let excerpt = if row.excerpt.is_empty() {
        None
    } else {
        Some(row.excerpt)
    };
    let folder = if row.folder.is_empty() {
        None
    } else {
        Some(row.folder)
    };
    let cover = if row.cover.is_empty() {
        None
    } else {
        Some(row.cover)
    };

    let tags = parse_tags(&row.tags);
    let highlights = parse_highlights(&row.highlights);
    let created = parse_timestamp(&row.created)?;

    Ok(RaindropItem {
        id: row.id,
        title: row.title,
        note,
        excerpt,
        url: row.url,
        folder,
        tags,
        created,
        cover,
        highlights,
    })
}

/// Parse the comma-separated tags field.
///
/// Raindrop exports tags as `"tag1, tag2, tag3"` within a quoted CSV field.
fn parse_tags(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Parse the highlights field.
///
/// Raindrop encodes highlights as newline-separated text snippets.
fn parse_highlights(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    raw.lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Parse an ISO 8601 timestamp string, returning `None` for empty strings.
fn parse_timestamp(raw: &str) -> Result<Option<OffsetDateTime>, ImportError> {
    if raw.is_empty() {
        return Ok(None);
    }
    OffsetDateTime::parse(raw, &Iso8601::DEFAULT)
        .map(Some)
        .map_err(|_| ImportError::Timestamp(raw.to_owned()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SAMPLE_CSV: &str = r#"id,title,note,excerpt,url,folder,tags,created,cover,highlights
12345,"My Article","A note about it","Short excerpt","https://example.com/article","Reading List","rust, programming","2024-05-28T13:00:00Z","https://example.com/cover.jpg","First highlight
Second highlight"
67890,"Untitled","","","https://example.com/other","","","","",""
"#;

    #[test]
    fn parse_csv_basic() {
        let items = parse_raindrop_csv(SAMPLE_CSV.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);

        let first = &items[0];
        assert_eq!(first.id, "12345");
        assert_eq!(first.title, "My Article");
        assert_eq!(first.note.as_deref(), Some("A note about it"));
        assert_eq!(first.excerpt.as_deref(), Some("Short excerpt"));
        assert_eq!(first.url, "https://example.com/article");
        assert_eq!(first.folder.as_deref(), Some("Reading List"));
        assert_eq!(first.tags, vec!["rust", "programming"]);
        assert!(first.created.is_some());
        assert_eq!(
            first.cover.as_deref(),
            Some("https://example.com/cover.jpg")
        );
        assert_eq!(first.highlights.len(), 2);
    }

    #[test]
    fn parse_csv_empty_fields() {
        let items = parse_raindrop_csv(SAMPLE_CSV.as_bytes()).unwrap();
        let second = &items[1];
        assert_eq!(second.id, "67890");
        assert_eq!(second.title, "Untitled");
        assert!(second.note.is_none());
        assert!(second.excerpt.is_none());
        assert!(second.folder.is_none());
        assert!(second.tags.is_empty());
        assert!(second.created.is_none());
        assert!(second.cover.is_none());
        assert!(second.highlights.is_empty());
    }

    #[test]
    fn parse_tags_splitting() {
        assert_eq!(parse_tags("a, b, c"), vec!["a", "b", "c"]);
        assert_eq!(parse_tags("single"), vec!["single"]);
        assert!(parse_tags("").is_empty());
    }

    #[test]
    fn parse_timestamp_valid() {
        let ts = parse_timestamp("2024-05-28T13:00:00Z").unwrap();
        assert!(ts.is_some());
    }

    #[test]
    fn parse_timestamp_empty() {
        let ts = parse_timestamp("").unwrap();
        assert!(ts.is_none());
    }

    #[test]
    fn parse_timestamp_invalid() {
        let result = parse_timestamp("not-a-date");
        assert!(result.is_err());
    }
}
