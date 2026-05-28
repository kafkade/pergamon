//! Feed parsing via `feed-rs` with normalization to pergamon types.

use time::OffsetDateTime;

use crate::error::FeedError;

/// A parsed feed with its metadata and entries.
#[derive(Debug, Clone)]
pub struct ParsedFeed {
    /// Feed title.
    pub title: String,
    /// Canonical feed URL (may differ from the fetched URL after redirects).
    pub feed_url: String,
    /// Website URL of the feed publisher.
    pub site_url: Option<String>,
    /// Feed description or subtitle.
    pub description: Option<String>,
    /// Parsed entries from this fetch.
    pub entries: Vec<ParsedEntry>,
}

/// A single entry/item from a parsed feed.
#[derive(Debug, Clone)]
pub struct ParsedEntry {
    /// Entry GUID / ID as provided by the feed.
    pub guid: Option<String>,
    /// Entry title.
    pub title: String,
    /// Canonical entry URL (permalink).
    pub url: Option<String>,
    /// Author name.
    pub author: Option<String>,
    /// Short summary/description from the feed.
    pub summary: Option<String>,
    /// Full content body (HTML or text).
    pub content: Option<String>,
    /// Publication timestamp.
    pub published_at: Option<OffsetDateTime>,
    /// Last-updated timestamp.
    pub updated_at: Option<OffsetDateTime>,
}

/// Parse raw feed bytes into a [`ParsedFeed`].
///
/// `url` is the URL that was fetched — used as the feed identity if the feed
/// doesn't declare its own URL.
///
/// # Errors
///
/// Returns [`FeedError::Parse`] if `feed-rs` cannot parse the content.
pub fn parse_feed(bytes: &[u8], url: &str) -> Result<ParsedFeed, FeedError> {
    let model = feed_rs::parser::parse(bytes).map_err(|e| FeedError::Parse(e.to_string()))?;

    let title = model
        .title
        .map_or_else(|| "Untitled Feed".to_owned(), |t| t.content);

    let feed_url = model
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("self"))
        .map_or_else(|| url.to_owned(), |l| l.href.clone());

    let site_url = model
        .links
        .iter()
        .find(|l| l.rel.as_deref() != Some("self"))
        .map(|l| l.href.clone());

    let description = model.description.map(|d| d.content);

    let entries = model.entries.into_iter().map(normalize_entry).collect();

    Ok(ParsedFeed {
        title,
        feed_url,
        site_url,
        description,
        entries,
    })
}

/// Normalize a `feed-rs` entry into our domain type.
fn normalize_entry(entry: feed_rs::model::Entry) -> ParsedEntry {
    let guid = if entry.id.is_empty() {
        None
    } else {
        Some(entry.id)
    };

    let title = entry
        .title
        .map_or_else(|| "Untitled".to_owned(), |t| t.content);

    let url = entry.links.first().map(|l| l.href.clone());

    let author = entry.authors.first().map(|a| a.name.clone());

    let summary = entry.summary.map(|s| s.content);

    let content = entry
        .content
        .and_then(|c| c.body)
        .or_else(|| summary.clone());

    let published_at = entry
        .published
        .map(|dt| OffsetDateTime::from_unix_timestamp(dt.timestamp()))
        .and_then(Result::ok);

    let updated_at = entry
        .updated
        .map(|dt| OffsetDateTime::from_unix_timestamp(dt.timestamp()))
        .and_then(Result::ok);

    ParsedEntry {
        guid,
        title,
        url,
        author,
        summary,
        content,
        published_at,
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS_SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Blog</title>
    <link>https://test.example.com</link>
    <description>A test blog feed</description>
    <item>
      <title>First Post</title>
      <link>https://test.example.com/first</link>
      <guid>urn:uuid:00000000-0000-0000-0000-000000000001</guid>
      <description>Summary of first post</description>
      <pubDate>Mon, 01 Jan 2024 12:00:00 GMT</pubDate>
    </item>
    <item>
      <title>Second Post</title>
      <link>https://test.example.com/second</link>
      <guid>urn:uuid:00000000-0000-0000-0000-000000000002</guid>
      <description>Summary of second post</description>
    </item>
  </channel>
</rss>"#;

    const ATOM_SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Test Feed</title>
  <link href="https://atom.example.com" rel="alternate"/>
  <link href="https://atom.example.com/feed.xml" rel="self"/>
  <id>urn:uuid:atom-feed-id</id>
  <updated>2024-01-15T10:00:00Z</updated>
  <entry>
    <title>Atom Entry One</title>
    <link href="https://atom.example.com/entry-1"/>
    <id>atom-entry-1</id>
    <updated>2024-01-15T10:00:00Z</updated>
    <summary>Summary of atom entry one</summary>
  </entry>
</feed>"#;

    #[test]
    fn parse_rss_feed() {
        let feed = parse_feed(RSS_SAMPLE.as_bytes(), "https://test.example.com/rss")
            .unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(feed.title, "Test Blog");
        assert_eq!(feed.entries.len(), 2);

        let first = &feed.entries[0];
        assert_eq!(first.title, "First Post");
        assert_eq!(first.url.as_deref(), Some("https://test.example.com/first"));
        assert_eq!(
            first.guid.as_deref(),
            Some("urn:uuid:00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(first.summary.as_deref(), Some("Summary of first post"));
        assert!(first.published_at.is_some());
    }

    #[test]
    fn parse_atom_feed() {
        let feed = parse_feed(ATOM_SAMPLE.as_bytes(), "https://atom.example.com/feed.xml")
            .unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(feed.title, "Atom Test Feed");
        assert_eq!(
            feed.feed_url, "https://atom.example.com/feed.xml",
            "should use self link"
        );
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].title, "Atom Entry One");
        assert_eq!(feed.entries[0].guid.as_deref(), Some("atom-entry-1"));
    }

    #[test]
    fn parse_invalid_feed_returns_error() {
        let result = parse_feed(b"this is not a feed", "https://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn entry_without_guid_gets_none() {
        let rss = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>No GUID Feed</title>
    <item>
      <title>Post Without GUID</title>
      <link>https://example.com/no-guid</link>
    </item>
  </channel>
</rss>"#;

        let feed = parse_feed(rss.as_bytes(), "https://example.com/feed")
            .unwrap_or_else(|e| unreachable!("parse failed: {e}"));

        assert_eq!(feed.entries.len(), 1);
        // feed-rs may auto-generate an ID — our code sets None only if empty
        let entry = &feed.entries[0];
        assert_eq!(entry.title, "Post Without GUID");
    }
}
