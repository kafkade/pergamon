//! Pocket HTML export parser.
//!
//! Pocket exports bookmarks as a Netscape bookmark file with `<DT><A>` entries.
//! Each `<A>` tag may have `HREF`, `ADD_DATE`, and `TAGS` attributes.

use scraper::{Html, Selector};
use time::OffsetDateTime;

use crate::error::ImportError;

/// A single item parsed from a Pocket HTML export.
#[derive(Debug, Clone)]
pub struct PocketItem {
    /// The bookmarked URL.
    pub url: String,
    /// Bookmark title (inner text of the `<A>` tag).
    pub title: String,
    /// When the item was added (from `ADD_DATE` unix timestamp).
    pub add_date: Option<OffsetDateTime>,
    /// Tags associated with the item.
    pub tags: Vec<String>,
}

/// Parse a Pocket HTML export from raw bytes.
///
/// Expects the standard Netscape bookmark file format that Pocket exports.
///
/// # Errors
///
/// Returns `ImportError::Utf8Str` if the bytes are not valid UTF-8,
/// or `ImportError::Html` if the HTML selector cannot be parsed.
pub fn parse_pocket_html(bytes: &[u8]) -> Result<Vec<PocketItem>, ImportError> {
    let html_str = std::str::from_utf8(bytes)?;
    let document = Html::parse_document(html_str);

    let a_selector = Selector::parse("a")
        .map_err(|e| ImportError::Html(format!("failed to parse selector: {e}")))?;

    let mut items = Vec::new();
    for element in document.select(&a_selector) {
        let href = match element.value().attr("href") {
            Some(h) if !h.is_empty() => h,
            _ => continue,
        };

        let title = element.text().collect::<String>();
        let title = if title.trim().is_empty() {
            href.to_owned()
        } else {
            title.trim().to_owned()
        };

        let add_date = element
            .value()
            .attr("add_date")
            .and_then(|ts| parse_unix_timestamp(ts).ok().flatten());

        let tags = element
            .value()
            .attr("tags")
            .map(parse_tags)
            .unwrap_or_default();

        items.push(PocketItem {
            url: href.to_owned(),
            title,
            add_date,
            tags,
        });
    }

    Ok(items)
}

/// Parse comma-separated tags.
fn parse_tags(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Parse a Unix timestamp (seconds since epoch) into an `OffsetDateTime`.
fn parse_unix_timestamp(raw: &str) -> Result<Option<OffsetDateTime>, ImportError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let secs: i64 = raw
        .parse()
        .map_err(|_| ImportError::Timestamp(raw.to_owned()))?;
    OffsetDateTime::from_unix_timestamp(secs)
        .map(Some)
        .map_err(|_| ImportError::Timestamp(raw.to_owned()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SAMPLE_HTML: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Pocket Export</TITLE>
<H1>Pocket Export</H1>
<DL><p>
    <DT><A HREF="https://example.com/article1" ADD_DATE="1680990446" TAGS="reading,list">Article Title 1</A>
    <DT><A HREF="https://example.com/article2" ADD_DATE="1681090556">Article Title 2</A>
    <DT><A HREF="https://example.com/article3" ADD_DATE="1681190666" TAGS="technology">Article Title 3</A>
</DL><p>
"#;

    #[test]
    fn parse_html_basic() {
        let items = parse_pocket_html(SAMPLE_HTML.as_bytes()).unwrap();
        assert_eq!(items.len(), 3);

        let first = &items[0];
        assert_eq!(first.url, "https://example.com/article1");
        assert_eq!(first.title, "Article Title 1");
        assert!(first.add_date.is_some());
        assert_eq!(first.tags, vec!["reading", "list"]);
    }

    #[test]
    fn parse_html_no_tags() {
        let items = parse_pocket_html(SAMPLE_HTML.as_bytes()).unwrap();
        let second = &items[1];
        assert_eq!(second.url, "https://example.com/article2");
        assert!(second.tags.is_empty());
    }

    #[test]
    fn parse_html_timestamp() {
        let items = parse_pocket_html(SAMPLE_HTML.as_bytes()).unwrap();
        let first = &items[0];
        assert!(first.add_date.is_some());
        let ts = first.add_date.unwrap();
        assert_eq!(ts.year(), 2023);
    }

    #[test]
    fn parse_empty_html() {
        let html = b"<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><p></DL><p>";
        let items = parse_pocket_html(html).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn parse_unix_timestamp_valid() {
        let ts = parse_unix_timestamp("1680990446").unwrap();
        assert!(ts.is_some());
    }

    #[test]
    fn parse_unix_timestamp_empty() {
        let ts = parse_unix_timestamp("").unwrap();
        assert!(ts.is_none());
    }

    #[test]
    fn parse_unix_timestamp_invalid() {
        let result = parse_unix_timestamp("abc");
        assert!(result.is_err());
    }
}
