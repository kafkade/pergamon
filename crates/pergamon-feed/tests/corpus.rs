//! Corpus-based integration tests for feed parsing.
//!
//! Each fixture exercises a different feed format, extension, or edge case.
//! Tests verify that `parse_feed` doesn't panic and produces the expected
//! structure, but deliberately avoid asserting on every field — fixtures are
//! for *robustness* coverage, not pixel-perfect output.

use pergamon_feed::{ParsedFeed, parse_feed};

// ── Helpers ──────────────────────────────────────────────────────────

/// Parse a fixture from bytes and a fake URL, returning the feed or
/// panicking with a diagnostic message.
fn parse_fixture(bytes: &[u8], url: &str) -> ParsedFeed {
    parse_feed(bytes, url).unwrap_or_else(|e| {
        unreachable!("Failed to parse fixture {url}: {e}");
    })
}

// ── RSS Fixtures ─────────────────────────────────────────────────────

#[test]
fn rss_blog_simple() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/blog-simple.xml"),
        "https://thoughtful.engineering/feed.xml",
    );
    assert_eq!(feed.title, "Thoughtful Engineering");
    assert!(feed.site_url.is_some());
    assert_eq!(feed.entries.len(), 3);
    // First entry has all standard fields.
    let first = &feed.entries[0];
    assert!(!first.title.is_empty());
    assert!(first.url.is_some());
    assert!(first.guid.is_some());
    assert!(first.published_at.is_some());
}

#[test]
fn rss_blog_with_cdata() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/blog-with-cdata.xml"),
        "https://devnotes.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 2);
    // CDATA titles should be unwrapped to plain text.
    let first = &feed.entries[0];
    assert!(
        !first.title.is_empty(),
        "CDATA title should be extracted as text"
    );
    // Content should contain the HTML from CDATA.
    assert!(
        first.summary.is_some() || first.content.is_some(),
        "CDATA description or content should be present"
    );
}

#[test]
fn rss_news_full() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/news-full.xml"),
        "https://globaltechreport.com/feed.xml",
    );
    assert_eq!(feed.title, "Global Tech Report");
    assert_eq!(feed.entries.len(), 5);
    // First entry uses content:encoded and dc:creator.
    let first = &feed.entries[0];
    assert!(first.author.is_some(), "dc:creator should be extracted");
    assert!(
        first.content.is_some(),
        "content:encoded should be extracted"
    );
}

#[test]
fn rss_podcast() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/podcast.xml"),
        "https://systemsandsignals.fm/feed.xml",
    );
    assert!(feed.title.contains("Systems"));
    assert_eq!(feed.entries.len(), 3);
    // Podcast episodes should still parse as entries.
    let ep = &feed.entries[0];
    assert!(ep.title.contains("Episode 42"));
    assert!(ep.url.is_some());
}

#[test]
fn rss_minimal() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/minimal.xml"),
        "https://minimal.example.com/feed.xml",
    );
    assert_eq!(feed.title, "Minimal Feed");
    assert_eq!(feed.entries.len(), 1);
    let item = &feed.entries[0];
    assert_eq!(item.title, "Single Item");
    // Minimal item: no URL, no GUID, no date, no content.
    assert!(item.url.is_none());
    assert!(item.published_at.is_none());
}

#[test]
fn rss_no_guid() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/no-guid.xml"),
        "https://noguid.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 3);
    // Items without explicit <guid> — feed-rs may synthesize one or leave None.
    // At minimum, entries should parse without error.
    let with_link = &feed.entries[0];
    assert!(with_link.url.is_some());
}

#[test]
fn rss_html_in_title() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/html-in-title.xml"),
        "https://entities.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 3);
    // Titles with HTML entities should be decoded.
    let first = &feed.entries[0];
    assert!(
        first.title.contains('"') || first.title.contains("&quot;"),
        "Entities in title should be decoded or preserved"
    );
}

#[test]
fn rss_empty_fields() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/empty-fields.xml"),
        "https://sparse.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 3);
    // Should not panic on empty fields.
}

#[test]
fn rss_many_items() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/many-items.xml"),
        "https://highvolume.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 25);
    // All items should have GUIDs.
    for entry in &feed.entries {
        assert!(entry.guid.is_some());
    }
}

#[test]
fn rss_relative_urls() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/relative-urls.xml"),
        "https://relative.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 3);
    // Relative URLs are preserved as-is by feed-rs (no resolution).
    // Downstream code resolves them.
    let first = &feed.entries[0];
    assert!(first.url.is_some());
}

#[test]
fn rss_namespaces() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/namespaces.xml"),
        "https://nsrich.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 2);
    // dc:creator should be extracted as author.
    let first = &feed.entries[0];
    assert!(
        first.author.is_some(),
        "dc:creator should map to author field"
    );
    // content:encoded should produce content.
    assert!(
        first.content.is_some(),
        "content:encoded should produce content"
    );
}

#[test]
fn rss_no_dates() {
    let feed = parse_fixture(
        include_bytes!("fixtures/rss/no-dates.xml"),
        "https://dateless.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 2);
    for entry in &feed.entries {
        assert!(
            entry.published_at.is_none(),
            "Items without pubDate should have None published_at"
        );
    }
}

// ── Atom Fixtures ────────────────────────────────────────────────────

#[test]
fn atom_blog() {
    let feed = parse_fixture(
        include_bytes!("fixtures/atom/blog.xml"),
        "https://fpweekly.example.com/feed.xml",
    );
    assert_eq!(feed.title, "Functional Programming Weekly");
    assert_eq!(feed.entries.len(), 3);
    let first = &feed.entries[0];
    assert!(first.guid.is_some());
    assert!(first.published_at.is_some());
    assert!(first.author.is_some());
}

#[test]
fn atom_github_releases() {
    let feed = parse_fixture(
        include_bytes!("fixtures/atom/github-releases.xml"),
        "https://github.com/example-org/example-lib/releases.atom",
    );
    assert!(feed.title.contains("example-lib"));
    assert_eq!(feed.entries.len(), 3);
    // Release entries should have content.
    let first = &feed.entries[0];
    assert_eq!(first.title, "v2.1.0");
    assert!(first.content.is_some());
}

#[test]
fn atom_xhtml_content() {
    let feed = parse_fixture(
        include_bytes!("fixtures/atom/xhtml-content.xml"),
        "https://xhtml.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 1);
    let entry = &feed.entries[0];
    // XHTML content should be extracted (as serialized HTML or text).
    assert!(
        entry.content.is_some() || entry.summary.is_some(),
        "XHTML content should be extracted"
    );
}

#[test]
fn atom_multiple_links() {
    let feed = parse_fixture(
        include_bytes!("fixtures/atom/multiple-links.xml"),
        "https://multilink.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 1);
    // Parser should pick the alternate link as the entry URL.
    let entry = &feed.entries[0];
    assert!(entry.url.is_some());
}

#[test]
fn atom_categories() {
    let feed = parse_fixture(
        include_bytes!("fixtures/atom/categories.xml"),
        "https://tagged.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 2);
    // Categories are not mapped to ParsedEntry fields, but parsing should succeed.
}

#[test]
fn atom_minimal() {
    let feed = parse_fixture(
        include_bytes!("fixtures/atom/minimal.xml"),
        "https://minimal-atom.example.com/feed.xml",
    );
    assert_eq!(feed.title, "Minimal Atom");
    assert_eq!(feed.entries.len(), 1);
    let entry = &feed.entries[0];
    assert_eq!(entry.title, "One Entry");
    assert!(entry.url.is_none());
}

// ── JSON Feed Fixtures ───────────────────────────────────────────────

#[test]
fn json_feed_blog() {
    let feed = parse_fixture(
        include_bytes!("fixtures/json/blog.json"),
        "https://indieweb.example.com/feed.json",
    );
    assert_eq!(feed.title, "Indie Web Notes");
    assert_eq!(feed.entries.len(), 3);
    let first = &feed.entries[0];
    assert!(first.url.is_some());
    assert!(first.published_at.is_some());
    // JSON Feed provides both content_html and content_text; content should exist.
    assert!(first.content.is_some() || first.summary.is_some());
}

#[test]
fn json_feed_attachments() {
    let feed = parse_fixture(
        include_bytes!("fixtures/json/with-attachments.json"),
        "https://podcast-json.example.com/feed.json",
    );
    assert_eq!(feed.entries.len(), 2);
    // Attachments aren't mapped to ParsedEntry, but parsing should succeed.
    let first = &feed.entries[0];
    assert!(first.title.contains("Episode 1"));
}

#[test]
fn json_feed_minimal() {
    let feed = parse_fixture(
        include_bytes!("fixtures/json/minimal.json"),
        "https://minimal-json.example.com/feed.json",
    );
    assert_eq!(feed.title, "Minimal JSON Feed");
    assert_eq!(feed.entries.len(), 1);
}

// ── Edge-Case Fixtures ───────────────────────────────────────────────

#[test]
fn edge_malformed_dates() {
    let feed = parse_fixture(
        include_bytes!("fixtures/edge/malformed-dates.xml"),
        "https://baddates.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 5);
    // Items with unparseable dates should have None published_at rather
    // than causing a parse failure.
    let no_date_item = &feed.entries[4];
    assert!(
        no_date_item.published_at.is_none(),
        "Item without pubDate should have None published_at"
    );
}

#[test]
fn edge_utf8_multibyte() {
    let feed = parse_fixture(
        include_bytes!("fixtures/edge/utf8-multibyte.xml"),
        "https://multilang.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 5);
    // Japanese title should survive round-trip.
    let jp = &feed.entries[0];
    assert!(jp.title.contains("Rust"), "Japanese title should be intact");
    // Arabic entry should parse.
    let ar = &feed.entries[4];
    assert!(!ar.title.is_empty());
}

#[test]
fn edge_long_content() {
    let feed = parse_fixture(
        include_bytes!("fixtures/edge/long-content.xml"),
        "https://longcontent.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 1);
    let entry = &feed.entries[0];
    assert!(entry.content.is_some());
    let content = entry.content.as_deref().unwrap_or_default();
    // Content should be substantial (at least 1000 chars).
    assert!(
        content.len() > 1000,
        "Long content should be preserved (got {} bytes)",
        content.len()
    );
}

#[test]
fn edge_special_chars() {
    let feed = parse_fixture(
        include_bytes!("fixtures/edge/special-chars.xml"),
        "https://specialchars.example.com/feed.xml",
    );
    assert_eq!(feed.entries.len(), 4);
    // XML entities should be decoded.
    let first = &feed.entries[0];
    assert!(
        first.title.contains('&') || first.title.contains("&amp;"),
        "Ampersand should be preserved"
    );
}

// ── Summary test ─────────────────────────────────────────────────────

#[test]
#[allow(clippy::too_many_lines)]
fn all_fixtures_parse_without_panic() {
    // Meta-test: every fixture in the corpus should parse without panicking,
    // even if we don't assert on specific fields.
    let fixtures: &[(&[u8], &str)] = &[
        (
            include_bytes!("fixtures/rss/blog-simple.xml"),
            "https://example.com/rss1",
        ),
        (
            include_bytes!("fixtures/rss/blog-with-cdata.xml"),
            "https://example.com/rss2",
        ),
        (
            include_bytes!("fixtures/rss/news-full.xml"),
            "https://example.com/rss3",
        ),
        (
            include_bytes!("fixtures/rss/podcast.xml"),
            "https://example.com/rss4",
        ),
        (
            include_bytes!("fixtures/rss/minimal.xml"),
            "https://example.com/rss5",
        ),
        (
            include_bytes!("fixtures/rss/no-guid.xml"),
            "https://example.com/rss6",
        ),
        (
            include_bytes!("fixtures/rss/html-in-title.xml"),
            "https://example.com/rss7",
        ),
        (
            include_bytes!("fixtures/rss/empty-fields.xml"),
            "https://example.com/rss8",
        ),
        (
            include_bytes!("fixtures/rss/many-items.xml"),
            "https://example.com/rss9",
        ),
        (
            include_bytes!("fixtures/rss/relative-urls.xml"),
            "https://example.com/rss10",
        ),
        (
            include_bytes!("fixtures/rss/namespaces.xml"),
            "https://example.com/rss11",
        ),
        (
            include_bytes!("fixtures/rss/no-dates.xml"),
            "https://example.com/rss12",
        ),
        (
            include_bytes!("fixtures/atom/blog.xml"),
            "https://example.com/atom1",
        ),
        (
            include_bytes!("fixtures/atom/github-releases.xml"),
            "https://example.com/atom2",
        ),
        (
            include_bytes!("fixtures/atom/xhtml-content.xml"),
            "https://example.com/atom3",
        ),
        (
            include_bytes!("fixtures/atom/multiple-links.xml"),
            "https://example.com/atom4",
        ),
        (
            include_bytes!("fixtures/atom/categories.xml"),
            "https://example.com/atom5",
        ),
        (
            include_bytes!("fixtures/atom/minimal.xml"),
            "https://example.com/atom6",
        ),
        (
            include_bytes!("fixtures/json/blog.json"),
            "https://example.com/json1",
        ),
        (
            include_bytes!("fixtures/json/with-attachments.json"),
            "https://example.com/json2",
        ),
        (
            include_bytes!("fixtures/json/minimal.json"),
            "https://example.com/json3",
        ),
        (
            include_bytes!("fixtures/edge/malformed-dates.xml"),
            "https://example.com/edge1",
        ),
        (
            include_bytes!("fixtures/edge/utf8-multibyte.xml"),
            "https://example.com/edge2",
        ),
        (
            include_bytes!("fixtures/edge/long-content.xml"),
            "https://example.com/edge3",
        ),
        (
            include_bytes!("fixtures/edge/special-chars.xml"),
            "https://example.com/edge4",
        ),
    ];

    for (i, (bytes, url)) in fixtures.iter().enumerate() {
        let result = parse_feed(bytes, url);
        if let Err(e) = &result {
            unreachable!("Fixture {i} ({url}) failed to parse: {e}");
        }
        let feed = result.unwrap_or_else(|_| unreachable!());
        assert!(
            !feed.title.is_empty(),
            "Fixture {i} ({url}) has empty title"
        );
    }
}
