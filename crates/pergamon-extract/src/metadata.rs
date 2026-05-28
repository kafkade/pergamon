//! Metadata extraction from HTML documents.
//!
//! Extracts structured metadata from Open Graph tags, Twitter Cards,
//! `<meta>` tags, `<title>`, and `<link rel="canonical">`.

use scraper::{Html, Selector};

/// Metadata extracted from an HTML document's `<head>`.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    /// Page title (from OG, then `<title>`).
    pub title: Option<String>,
    /// Author name.
    pub author: Option<String>,
    /// Description or excerpt.
    pub description: Option<String>,
    /// Canonical URL.
    pub canonical_url: Option<String>,
    /// Open Graph image URL.
    pub og_image: Option<String>,
    /// Site name (from `og:site_name`).
    pub site_name: Option<String>,
    /// Published time (raw string from `article:published_time`).
    pub published_time: Option<String>,
}

/// Extract metadata from an HTML string.
///
/// Looks for Open Graph tags, standard meta tags, `<title>`, and
/// `<link rel="canonical">`.
#[must_use]
pub fn extract_metadata(html: &str) -> Metadata {
    let document = Html::parse_document(html);

    let description =
        og_content(&document, "og:description").or_else(|| meta_content(&document, "description"));
    let canonical_url = og_content(&document, "og:url").or_else(|| canonical_link(&document));
    let author =
        meta_content(&document, "author").or_else(|| og_content(&document, "article:author"));

    let mut title = og_content(&document, "og:title");
    if title.is_none() {
        title = title_tag(&document);
    }

    Metadata {
        title,
        author,
        description,
        canonical_url,
        og_image: og_content(&document, "og:image"),
        site_name: og_content(&document, "og:site_name"),
        published_time: og_content(&document, "article:published_time"),
    }
}

/// Get `content` from `<meta property="..." content="...">`.
fn og_content(doc: &Html, property: &str) -> Option<String> {
    let selector_str = format!("meta[property=\"{property}\"]");
    let selector = Selector::parse(&selector_str).ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Get `content` from `<meta name="..." content="...">`.
fn meta_content(doc: &Html, name: &str) -> Option<String> {
    let selector_str = format!("meta[name=\"{name}\"]");
    let selector = Selector::parse(&selector_str).ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Get href from `<link rel="canonical" href="...">`.
fn canonical_link(doc: &Html) -> Option<String> {
    let selector = Selector::parse("link[rel=\"canonical\"]").ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("href"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Get text from `<title>`.
fn title_tag(doc: &Html) -> Option<String> {
    let selector = Selector::parse("title").ok()?;
    let text: String = doc
        .select(&selector)
        .next()?
        .text()
        .collect::<String>()
        .trim()
        .to_owned();
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_og_metadata() {
        let html = r#"
        <html>
        <head>
            <meta property="og:title" content="Test Article" />
            <meta property="og:description" content="A test description" />
            <meta property="og:image" content="https://example.com/image.jpg" />
            <meta property="og:url" content="https://example.com/article" />
            <meta property="og:site_name" content="Example Site" />
            <meta property="article:published_time" content="2024-01-15T10:00:00Z" />
            <meta name="author" content="Jane Doe" />
            <title>Fallback Title</title>
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.title.as_deref(), Some("Test Article"));
        assert_eq!(meta.description.as_deref(), Some("A test description"));
        assert_eq!(
            meta.og_image.as_deref(),
            Some("https://example.com/image.jpg")
        );
        assert_eq!(
            meta.canonical_url.as_deref(),
            Some("https://example.com/article")
        );
        assert_eq!(meta.site_name.as_deref(), Some("Example Site"));
        assert_eq!(meta.author.as_deref(), Some("Jane Doe"));
        assert_eq!(meta.published_time.as_deref(), Some("2024-01-15T10:00:00Z"));
    }

    #[test]
    fn falls_back_to_title_tag() {
        let html = r"
        <html>
        <head><title>Page Title</title></head>
        <body></body>
        </html>
        ";

        let meta = extract_metadata(html);
        assert_eq!(meta.title.as_deref(), Some("Page Title"));
    }

    #[test]
    fn falls_back_to_meta_description() {
        let html = r#"
        <html>
        <head><meta name="description" content="Meta desc" /></head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.description.as_deref(), Some("Meta desc"));
    }

    #[test]
    fn canonical_link_extraction() {
        let html = r#"
        <html>
        <head><link rel="canonical" href="https://example.com/canonical" /></head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(
            meta.canonical_url.as_deref(),
            Some("https://example.com/canonical")
        );
    }

    #[test]
    fn empty_html_returns_defaults() {
        let meta = extract_metadata("<html><head></head><body></body></html>");
        assert!(meta.title.is_none());
        assert!(meta.author.is_none());
        assert!(meta.description.is_none());
    }
}
