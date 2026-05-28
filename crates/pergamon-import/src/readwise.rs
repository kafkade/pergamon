//! Readwise CSV/JSON export parser.
//!
//! Parses Readwise highlight exports in CSV format. The CSV uses
//! flexible headers — column names are matched case-insensitively
//! and unknown columns are silently ignored.

use time::OffsetDateTime;
use time::format_description::well_known::Iso8601;

use crate::error::ImportError;

/// A single highlight parsed from a Readwise export.
#[derive(Debug, Clone)]
pub struct ReadwiseItem {
    /// Title of the source (book, article, podcast, etc.).
    pub title: String,
    /// Author of the source.
    pub author: Option<String>,
    /// Source type as reported by Readwise (e.g. "book", "article").
    pub source_type: Option<String>,
    /// Category (e.g. "books", "articles", "tweets", "podcasts").
    pub category: Option<String>,
    /// The highlighted text.
    pub highlight: String,
    /// User note attached to the highlight.
    pub note: Option<String>,
    /// Tags on the highlight.
    pub tags: Vec<String>,
    /// Location in the source (Kindle location, page number, etc.).
    pub location: Option<String>,
    /// When the highlight was created.
    pub highlighted_at: Option<OffsetDateTime>,
    /// URL of the source (for articles, web content).
    pub source_url: Option<String>,
    /// Tags on the source book/article.
    pub book_tags: Vec<String>,
    /// Readwise's internal UUID for this highlight.
    pub uuid: Option<String>,
}

/// Parse a Readwise CSV export from raw bytes.
///
/// Uses flexible header matching: column names are lowercased, spaces are
/// replaced with underscores, and unknown columns are ignored. The only
/// required columns are `title` and `highlight`.
///
/// # Errors
///
/// Returns `ImportError::Csv` if the CSV cannot be parsed, or
/// `ImportError::Timestamp` for invalid date values.
pub fn parse_readwise_csv(bytes: &[u8]) -> Result<Vec<ReadwiseItem>, ImportError> {
    let text = decode_utf8_bom(bytes)?;

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());

    // Build a mapping from normalized header names to column indices.
    let headers = reader.headers()?.clone();
    let col_map = build_column_map(&headers);

    // Require at minimum title and highlight columns.
    let title_col = col_map.get("title").or_else(|| col_map.get("book_title"));
    let highlight_col = col_map.get("highlight");

    let title_idx = *title_col.ok_or_else(|| {
        ImportError::Html("missing required CSV column: Title or Book Title".to_owned())
    })?;
    let highlight_idx = *highlight_col
        .ok_or_else(|| ImportError::Html("missing required CSV column: Highlight".to_owned()))?;

    let mut items = Vec::new();
    for result in reader.records() {
        let record = result?;

        let title = field(&record, title_idx);
        let highlight = field(&record, highlight_idx);

        // Skip rows with empty title or highlight.
        if title.is_empty() || highlight.is_empty() {
            continue;
        }

        let author = opt_field(&record, col_map.get("author").copied());
        let source_type = opt_field(&record, col_map.get("source_type").copied());
        let category = opt_field(&record, col_map.get("category").copied());
        let note = opt_field(&record, col_map.get("note").copied());
        let location = opt_field(&record, col_map.get("location").copied());
        let source_url = opt_field(&record, col_map.get("source_url").copied())
            .or_else(|| opt_field(&record, col_map.get("url").copied()));
        let uuid = opt_field(&record, col_map.get("uuid").copied());

        let tags = opt_field(&record, col_map.get("tags").copied())
            .map(|s| parse_tags(&s))
            .unwrap_or_default();
        let book_tags = opt_field(&record, col_map.get("book_tags").copied())
            .map(|s| parse_tags(&s))
            .unwrap_or_default();

        let highlighted_at = opt_field(&record, col_map.get("highlighted_at").copied())
            .and_then(|s| parse_timestamp(&s));

        items.push(ReadwiseItem {
            title,
            author,
            source_type,
            category,
            highlight,
            note,
            tags,
            location,
            highlighted_at,
            source_url,
            book_tags,
            uuid,
        });
    }

    Ok(items)
}

/// Generate a stable synthetic URL for a Readwise source (for dedup).
///
/// Prefers the actual source URL when available; falls back to a hash
/// of title + author + source type.
#[must_use]
pub fn readwise_source_key(
    source_url: Option<&str>,
    title: &str,
    author: Option<&str>,
    source_type: Option<&str>,
) -> String {
    if let Some(url) = source_url {
        let url = url.trim();
        if !url.is_empty() {
            return url.to_owned();
        }
    }
    let norm_title = title.trim().to_lowercase();
    let norm_author = author.map_or(String::new(), |a| a.trim().to_lowercase());
    let norm_type = source_type.map_or(String::new(), |t| t.trim().to_lowercase());
    let mut hash = 0u64;
    for b in norm_title.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    hash = hash.wrapping_mul(31).wrapping_add(0xFF);
    for b in norm_author.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    hash = hash.wrapping_mul(31).wrapping_add(0xFE);
    for b in norm_type.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    format!("readwise://source/{hash:016x}")
}

/// Generate a stable synthetic URL for a Readwise highlight (for dedup).
///
/// Prefers the Readwise UUID if available; falls back to a hash of
/// source key + highlight text.
#[must_use]
pub fn readwise_highlight_key(
    uuid: Option<&str>,
    source_url: Option<&str>,
    title: &str,
    author: Option<&str>,
    source_type: Option<&str>,
    highlight_text: &str,
) -> String {
    if let Some(uuid) = uuid {
        let uuid = uuid.trim();
        if !uuid.is_empty() {
            return format!("readwise://highlight/{uuid}");
        }
    }
    let source = readwise_source_key(source_url, title, author, source_type);
    let norm_quote = highlight_text.trim().to_lowercase();
    let mut hash = 0u64;
    for b in source.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    for b in norm_quote.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    format!("readwise://highlight/{hash:016x}")
}

// ======================================================================
// Internal helpers
// ======================================================================

/// Decode bytes as UTF-8, stripping a BOM if present.
fn decode_utf8_bom(bytes: &[u8]) -> Result<&str, ImportError> {
    let stripped = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    Ok(std::str::from_utf8(stripped)?)
}

/// Normalize a header name: lowercase, replace spaces with underscores.
fn normalize_header(name: &str) -> String {
    name.trim().to_lowercase().replace(' ', "_")
}

/// Build a map from normalized header names to column indices.
fn build_column_map(headers: &csv::StringRecord) -> std::collections::HashMap<String, usize> {
    let mut map = std::collections::HashMap::new();
    for (idx, header) in headers.iter().enumerate() {
        let key = normalize_header(header);
        if !key.is_empty() {
            map.insert(key, idx);
        }
    }
    map
}

/// Get a field value from a CSV record, returning empty string if out of bounds.
fn field(record: &csv::StringRecord, idx: usize) -> String {
    record.get(idx).unwrap_or("").trim().to_owned()
}

/// Get an optional field value.
fn opt_field(record: &csv::StringRecord, idx: Option<usize>) -> Option<String> {
    idx.and_then(|i| {
        let val = record.get(i).unwrap_or("").trim();
        if val.is_empty() {
            None
        } else {
            Some(val.to_owned())
        }
    })
}

/// Parse comma-separated tags, trimming whitespace and filtering empties.
fn parse_tags(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Parse an ISO 8601 or common date-time string into `OffsetDateTime`.
fn parse_timestamp(raw: &str) -> Option<OffsetDateTime> {
    if raw.is_empty() {
        return None;
    }
    // Try ISO 8601 first
    if let Ok(dt) = OffsetDateTime::parse(raw, &Iso8601::DEFAULT) {
        return Some(dt);
    }
    // Try "YYYY-MM-DD HH:MM:SS" (naive, assume UTC)
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    if let Ok(pdt) = time::PrimitiveDateTime::parse(raw, &fmt) {
        return Some(pdt.assume_utc());
    }
    // Non-fatal: return None rather than failing the whole import
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SAMPLE_CSV: &str = "Title,Author,Source Type,Category,Highlight,Note,Tags,Location,Highlighted At,Source URL,Book Tags,UUID
\"The Great Gatsby\",\"F. Scott Fitzgerald\",\"book\",\"books\",\"So we beat on, boats against the current.\",\"Beautiful ending\",\"classics, fiction\",\"Page 180\",\"2024-05-27T22:21:26Z\",\"\",\"literature\",\"abc-123\"
\"Why We Sleep\",\"Matthew Walker\",\"book\",\"books\",\"Sleep is the single most effective thing.\",\"\",\"science\",\"Location 1234\",\"2024-06-01T10:00:00Z\",\"\",\"\",\"def-456\"
\"Some Article\",\"Jane Doe\",\"article\",\"articles\",\"Key insight from the article.\",\"Need to follow up\",\"tech\",\"\",\"2024-06-15T14:30:00Z\",\"https://example.com/article\",\"\",\"\"
";

    #[test]
    fn parse_csv_basic() {
        let items = parse_readwise_csv(SAMPLE_CSV.as_bytes()).unwrap();
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn fields_parsed_correctly() {
        let items = parse_readwise_csv(SAMPLE_CSV.as_bytes()).unwrap();
        let first = &items[0];
        assert_eq!(first.title, "The Great Gatsby");
        assert_eq!(first.author.as_deref(), Some("F. Scott Fitzgerald"));
        assert_eq!(first.source_type.as_deref(), Some("book"));
        assert_eq!(first.category.as_deref(), Some("books"));
        assert!(first.highlight.contains("boats against the current"));
        assert_eq!(first.note.as_deref(), Some("Beautiful ending"));
        assert_eq!(first.tags, vec!["classics", "fiction"]);
        assert_eq!(first.location.as_deref(), Some("Page 180"));
        assert!(first.highlighted_at.is_some());
        assert!(first.source_url.is_none()); // empty URL
        assert_eq!(first.book_tags, vec!["literature"]);
        assert_eq!(first.uuid.as_deref(), Some("abc-123"));
    }

    #[test]
    fn empty_fields_are_none() {
        let items = parse_readwise_csv(SAMPLE_CSV.as_bytes()).unwrap();
        let second = &items[1];
        assert!(second.note.is_none());
        assert!(second.source_url.is_none());
        assert!(second.book_tags.is_empty());
    }

    #[test]
    fn article_with_url() {
        let items = parse_readwise_csv(SAMPLE_CSV.as_bytes()).unwrap();
        let article = &items[2];
        assert_eq!(article.source_type.as_deref(), Some("article"));
        assert_eq!(
            article.source_url.as_deref(),
            Some("https://example.com/article")
        );
        assert!(article.uuid.is_none()); // empty UUID
    }

    #[test]
    fn flexible_headers_case_insensitive() {
        let csv = "TITLE,AUTHOR,HIGHLIGHT,NOTE\n\"Book\",\"Writer\",\"Some text\",\"A note\"\n";
        let items = parse_readwise_csv(csv.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Book");
        assert_eq!(items[0].highlight, "Some text");
    }

    #[test]
    fn flexible_headers_with_spaces() {
        let csv = "Title,Author,Source Type,Highlight,Highlighted At,Source URL,Book Tags\n\"Book\",\"Writer\",\"book\",\"Text\",\"2024-01-01T00:00:00Z\",\"https://example.com\",\"tag1\"\n";
        let items = parse_readwise_csv(csv.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_type.as_deref(), Some("book"));
        assert_eq!(items[0].source_url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn empty_csv() {
        let csv = "Title,Highlight\n";
        let items = parse_readwise_csv(csv.as_bytes()).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn skip_rows_with_empty_highlight() {
        let csv = "Title,Highlight\n\"Book\",\"\"\n\"Other\",\"Real text\"\n";
        let items = parse_readwise_csv(csv.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Other");
    }

    #[test]
    fn readwise_source_key_prefers_url() {
        let key = readwise_source_key(
            Some("https://example.com/article"),
            "Title",
            Some("Author"),
            Some("article"),
        );
        assert_eq!(key, "https://example.com/article");
    }

    #[test]
    fn readwise_source_key_fallback_hash() {
        let key = readwise_source_key(None, "The Great Gatsby", Some("Fitzgerald"), Some("book"));
        assert!(key.starts_with("readwise://source/"));
    }

    #[test]
    fn readwise_source_key_stable() {
        let k1 = readwise_source_key(None, "Title", Some("Author"), Some("book"));
        let k2 = readwise_source_key(None, "Title", Some("Author"), Some("book"));
        assert_eq!(k1, k2);
    }

    #[test]
    fn readwise_highlight_key_prefers_uuid() {
        let key = readwise_highlight_key(
            Some("abc-123"),
            None,
            "Title",
            Some("Author"),
            Some("book"),
            "text",
        );
        assert_eq!(key, "readwise://highlight/abc-123");
    }

    #[test]
    fn readwise_highlight_key_fallback_hash() {
        let key = readwise_highlight_key(
            None,
            None,
            "Title",
            Some("Author"),
            Some("book"),
            "Some text",
        );
        assert!(key.starts_with("readwise://highlight/"));
    }

    #[test]
    fn bom_handling() {
        let csv = "\u{feff}Title,Highlight\n\"Book\",\"Text\"\n";
        let items = parse_readwise_csv(csv.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn parse_tags_splitting() {
        assert_eq!(parse_tags("a, b, c"), vec!["a", "b", "c"]);
        assert_eq!(parse_tags("single"), vec!["single"]);
        assert!(parse_tags("").is_empty());
    }
}
