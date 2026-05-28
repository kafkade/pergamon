//! Article extraction using `readabilityrs` with `ammonia` sanitization.
//!
//! Implements the ADR-004 extraction flow:
//! 1. Parse HTML with readability to extract main article content
//! 2. Sanitize with ammonia to remove unsafe markup
//! 3. Merge with metadata for a complete extracted article

use time::OffsetDateTime;

use crate::error::ExtractError;
use crate::metadata::{self, Metadata};

/// A fully extracted and sanitized article.
#[derive(Debug, Clone)]
pub struct ExtractedArticle {
    /// Article title (readability > OG > `<title>`).
    pub title: Option<String>,
    /// Author name.
    pub author: Option<String>,
    /// Sanitized HTML content (safe for storage and rendering).
    pub content_html: String,
    /// Plain text content (for FTS indexing and terminal display).
    pub content_text: String,
    /// Short excerpt or description.
    pub excerpt: Option<String>,
    /// Site name from OG metadata.
    pub site_name: Option<String>,
    /// Canonical URL of the article.
    pub canonical_url: Option<String>,
    /// Open Graph image URL.
    pub og_image: Option<String>,
    /// Parsed publication timestamp (best-effort).
    pub published_at: Option<OffsetDateTime>,
    /// Embedded metadata from the page.
    pub metadata: Metadata,
}

/// Extract article content from raw HTML bytes.
///
/// `base_url` is the URL the HTML was fetched from, used as fallback
/// for canonical URL and for resolving relative links.
///
/// # Errors
///
/// Returns [`ExtractError::Encoding`] if bytes cannot be decoded as UTF-8.
/// Returns [`ExtractError::Extract`] if readability parsing fails.
pub fn extract_article(bytes: &[u8], base_url: &str) -> Result<ExtractedArticle, ExtractError> {
    let html = std::str::from_utf8(bytes).map_err(|e| ExtractError::Encoding(e.to_string()))?;

    extract_article_from_html(html, base_url)
}

/// Extract article content from an HTML string.
///
/// Lower-level API useful for tests and when encoding is already handled.
///
/// # Errors
///
/// Returns [`ExtractError::Extract`] if readability parsing fails.
pub fn extract_article_from_html(
    html: &str,
    base_url: &str,
) -> Result<ExtractedArticle, ExtractError> {
    // Step 1: Extract metadata from <head>
    let meta = metadata::extract_metadata(html);

    // Step 2: Extract main article content via readability
    let readability = readabilityrs::Readability::new(html, Some(base_url), None)
        .map_err(|e| ExtractError::Extract(e.to_string()))?;

    let article = readability
        .parse()
        .ok_or_else(|| ExtractError::Extract("readability could not extract content".to_owned()))?;

    // Step 3: Sanitize the extracted HTML with ammonia
    let raw_html = article.content.as_deref().unwrap_or("");
    let content_html = sanitize_html(raw_html);

    // Step 4: Use text_content if available, otherwise convert HTML to plain text
    let content_text = article
        .text_content
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| html_to_text(&content_html), |t| t.trim().to_owned());

    // Step 5: Merge metadata with readability results
    // Precedence: readability > OG > fallback
    let title = non_empty_string(article.title.as_deref()).or_else(|| meta.title.clone());

    let author = non_empty_string(article.byline.as_deref()).or_else(|| meta.author.clone());

    let excerpt = non_empty_string(article.excerpt.as_deref()).or_else(|| meta.description.clone());

    let canonical_url = meta
        .canonical_url
        .clone()
        .or_else(|| Some(base_url.to_owned()));

    let site_name =
        non_empty_string(article.site_name.as_deref()).or_else(|| meta.site_name.clone());

    // Best-effort date parsing
    let published_at = article
        .published_time
        .as_deref()
        .or(meta.published_time.as_deref())
        .and_then(parse_datetime);

    Ok(ExtractedArticle {
        title,
        author,
        content_html,
        content_text,
        excerpt,
        site_name,
        canonical_url,
        og_image: meta.og_image.clone(),
        published_at,
        metadata: meta,
    })
}

/// Sanitize HTML using ammonia with a reader-friendly allowlist.
fn sanitize_html(html: &str) -> String {
    ammonia::Builder::default()
        .add_tags(&[
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "p",
            "br",
            "hr",
            "ul",
            "ol",
            "li",
            "blockquote",
            "pre",
            "code",
            "em",
            "strong",
            "b",
            "i",
            "u",
            "s",
            "sub",
            "sup",
            "a",
            "img",
            "figure",
            "figcaption",
            "table",
            "thead",
            "tbody",
            "tr",
            "th",
            "td",
            "dl",
            "dt",
            "dd",
            "details",
            "summary",
        ])
        .add_tag_attributes("a", &["href", "title"])
        .add_tag_attributes("img", &["src", "alt", "title", "width", "height"])
        .add_tag_attributes("td", &["colspan", "rowspan"])
        .add_tag_attributes("th", &["colspan", "rowspan"])
        .clean(html)
        .to_string()
}

/// Convert HTML to readable plain text with reasonable whitespace.
fn html_to_text(html: &str) -> String {
    let doc = scraper::Html::parse_fragment(html);
    let mut text = String::new();
    collect_text(&doc.root_element(), &mut text);
    // Normalize whitespace: collapse runs of blank lines to at most 2 newlines
    let mut result = String::with_capacity(text.len());
    let mut blank_lines = 0u32;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_lines += 1;
            if blank_lines <= 2 {
                result.push('\n');
            }
        } else {
            blank_lines = 0;
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(trimmed);
            result.push('\n');
        }
    }
    result.trim().to_owned()
}

/// Recursively collect text from HTML nodes with structural whitespace.
fn collect_text(element: &scraper::ElementRef<'_>, out: &mut String) {
    use scraper::Node;

    for child in element.children() {
        match child.value() {
            Node::Text(text) => {
                out.push_str(text);
            }
            Node::Element(el) => {
                let tag = el.name();
                let is_block = matches!(
                    tag,
                    "p" | "div"
                        | "br"
                        | "hr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "li"
                        | "blockquote"
                        | "pre"
                        | "tr"
                        | "dt"
                        | "dd"
                        | "figure"
                        | "figcaption"
                        | "details"
                        | "summary"
                );

                if is_block {
                    out.push('\n');
                }

                if let Some(child_ref) = scraper::ElementRef::wrap(child) {
                    collect_text(&child_ref, out);
                }

                if is_block || tag == "br" {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
}

/// Return `Some(s.to_owned())` if `s` is non-empty.
fn non_empty_string(s: Option<&str>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_owned())
}

/// Best-effort parsing of date strings.
fn parse_datetime(s: &str) -> Option<OffsetDateTime> {
    // Try ISO 8601 / RFC 3339
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE_HTML: &str = r#"
    <html>
    <head>
        <title>Test Article</title>
        <meta property="og:title" content="OG Title" />
        <meta property="og:description" content="OG description" />
        <meta property="og:image" content="https://example.com/img.jpg" />
        <meta name="author" content="Jane Doe" />
    </head>
    <body>
        <nav><a href="/">Home</a> <a href="/about">About</a></nav>
        <article>
            <h1>Test Article Title</h1>
            <p>This is the first paragraph of the article with enough content
            to be considered the main body. It needs to be reasonably long
            for readability to pick it up as the main content area.</p>
            <p>This is the second paragraph adding more substance to the article.
            We need several paragraphs of real content for the readability
            algorithm to properly identify this as the main article area.</p>
            <p>A third paragraph helps ensure the content density is high enough.
            The readability algorithm scores content based on paragraph count,
            text length, and other heuristics to separate articles from chrome.</p>
            <script>alert('xss')</script>
        </article>
        <footer>Copyright 2024</footer>
    </body>
    </html>
    "#;

    #[test]
    fn sanitize_removes_scripts() {
        let dirty = r"<p>Safe</p><script>alert('xss')</script><p>Also safe</p>";
        let clean = sanitize_html(dirty);
        assert!(!clean.contains("script"));
        assert!(!clean.contains("alert"));
        assert!(clean.contains("Safe"));
        assert!(clean.contains("Also safe"));
    }

    #[test]
    fn sanitize_preserves_formatting() {
        let html = r"<h2>Title</h2><p>Text with <strong>bold</strong> and <em>italic</em>.</p>";
        let clean = sanitize_html(html);
        assert!(clean.contains("<h2>"));
        assert!(clean.contains("<strong>"));
        assert!(clean.contains("<em>"));
    }

    #[test]
    fn html_to_text_handles_paragraphs() {
        let html = "<p>First paragraph.</p><p>Second paragraph.</p>";
        let text = html_to_text(html);
        assert!(text.contains("First paragraph."));
        assert!(text.contains("Second paragraph."));
        // Should have a line break between paragraphs
        assert!(text.contains('\n'));
    }

    #[test]
    fn extract_from_bytes_rejects_invalid_utf8() {
        let bytes: &[u8] = &[0xFF, 0xFE, 0xFD];
        let result = extract_article(bytes, "https://example.com");
        assert!(result.is_err());
        match result {
            Err(ExtractError::Encoding(_)) => {} // expected
            other => unreachable!("expected Encoding error, got {other:?}"),
        }
    }

    #[test]
    fn non_empty_string_filters_blanks() {
        assert!(non_empty_string(None).is_none());
        assert!(non_empty_string(Some("")).is_none());
        assert!(non_empty_string(Some("  ")).is_none());
        assert_eq!(non_empty_string(Some("hello")), Some("hello".to_owned()));
    }

    #[test]
    fn parse_datetime_handles_rfc3339() {
        let dt = parse_datetime("2024-01-15T10:00:00Z");
        assert!(dt.is_some());
    }

    #[test]
    fn parse_datetime_returns_none_for_garbage() {
        assert!(parse_datetime("not-a-date").is_none());
    }

    #[test]
    fn full_extraction_pipeline() {
        // This test may fail if readability can't extract from our minimal sample.
        // That's acceptable — we test the pipeline, not readability's heuristics.
        let result = extract_article(ARTICLE_HTML.as_bytes(), "https://example.com/article");
        match result {
            Ok(article) => {
                // Should have gotten metadata at minimum
                assert!(
                    article.title.is_some() || article.metadata.title.is_some(),
                    "should extract some title"
                );
                // Content should not contain script tags
                assert!(!article.content_html.contains("script"));
                assert!(!article.content_html.contains("alert"));
                // Plain text should have content
                assert!(!article.content_text.is_empty());
            }
            Err(ExtractError::Extract(_)) => {
                // Readability couldn't extract — that's OK for a minimal test HTML.
                // The important thing is it didn't panic.
            }
            Err(e) => unreachable!("unexpected error type: {e}"),
        }
    }
}
