//! Metadata extraction from HTML documents.
//!
//! Extracts structured metadata from Open Graph tags, Twitter Cards,
//! `<meta>` tags, `<title>`, `<link rel="canonical">`, `<link rel="icon">`,
//! and JSON-LD (`<script type="application/ld+json">`).

use scraper::{Html, Selector};

/// Metadata extracted from an HTML document's `<head>`.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    /// Page title (from OG, Twitter Card, then `<title>`).
    pub title: Option<String>,
    /// Author name (from meta tags, JSON-LD, or Twitter creator).
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
    /// Favicon URL (from `<link rel="icon">` or variants). May be relative.
    pub favicon_url: Option<String>,
}

/// Extract metadata from an HTML string.
///
/// Looks for Open Graph tags, Twitter Cards, standard meta tags, JSON-LD,
/// `<title>`, `<link rel="canonical">`, and `<link rel="icon">`.
#[must_use]
pub fn extract_metadata(html: &str) -> Metadata {
    let document = Html::parse_document(html);

    // JSON-LD author extraction (runs first so OG/meta can override).
    let jsonld_author = extract_jsonld_author(&document);

    let description = og_content(&document, "og:description")
        .or_else(|| twitter_content(&document, "twitter:description"))
        .or_else(|| meta_content(&document, "description"));

    let canonical_url = og_content(&document, "og:url").or_else(|| canonical_link(&document));

    let author = meta_content(&document, "author")
        .or_else(|| og_content(&document, "article:author"))
        .or_else(|| twitter_content(&document, "twitter:creator"))
        .or(jsonld_author);

    let title = og_content(&document, "og:title")
        .or_else(|| twitter_content(&document, "twitter:title"))
        .or_else(|| title_tag(&document));

    let og_image =
        og_content(&document, "og:image").or_else(|| twitter_content(&document, "twitter:image"));

    let site_name = og_content(&document, "og:site_name")
        .or_else(|| twitter_content(&document, "twitter:site"));

    let favicon_url = extract_favicon(&document);

    Metadata {
        title,
        author,
        description,
        canonical_url,
        og_image,
        site_name,
        published_time: og_content(&document, "article:published_time"),
        favicon_url,
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

/// Get `content` from Twitter Card meta tags.
///
/// Twitter tags can appear as either `<meta name="twitter:*">` or
/// `<meta property="twitter:*">`, so we check both.
fn twitter_content(doc: &Html, name: &str) -> Option<String> {
    meta_content(doc, name).or_else(|| og_content(doc, name))
}

/// Extract favicon URL from `<link>` tags.
///
/// Checks `rel="icon"`, `rel="shortcut icon"`, and
/// `rel="apple-touch-icon"` in that order. Returns the raw `href`
/// which may be relative and needs resolving against the page URL.
fn extract_favicon(doc: &Html) -> Option<String> {
    // Try standard favicon link tags in priority order.
    for rel in &["icon", "shortcut icon", "apple-touch-icon"] {
        let selector_str = format!("link[rel=\"{rel}\"]");
        if let Ok(selector) = Selector::parse(&selector_str)
            && let Some(el) = doc.select(&selector).next()
            && let Some(href) = el.value().attr("href")
        {
            let href = href.trim();
            if !href.is_empty() {
                return Some(href.to_owned());
            }
        }
    }
    None
}

/// Extract author name from JSON-LD structured data.
///
/// Parses `<script type="application/ld+json">` blocks and looks for
/// `Article`, `NewsArticle`, `BlogPosting`, or `WebPage` types with
/// an `author` field. Handles `@graph` arrays and author as string,
/// object, or array.
fn extract_jsonld_author(doc: &Html) -> Option<String> {
    let selector = Selector::parse("script[type=\"application/ld+json\"]").ok()?;

    for element in doc.select(&selector) {
        let text: String = element.text().collect();
        let text = text.trim();
        if text.is_empty() {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
            && let Some(author) = find_author_in_jsonld(&value)
        {
            return Some(author);
        }
    }
    None
}

/// Recursively search a JSON-LD value for an author name.
fn find_author_in_jsonld(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(author) = find_author_in_jsonld(item) {
                    return Some(author);
                }
            }
            None
        }
        serde_json::Value::Object(obj) => {
            // Check @graph arrays.
            if let Some(graph) = obj.get("@graph")
                && let Some(author) = find_author_in_jsonld(graph)
            {
                return Some(author);
            }

            // Check if this object is an article-like type.
            if let Some(type_val) = obj.get("@type")
                && is_article_type(type_val)
            {
                return extract_author_from_object(obj);
            }

            None
        }
        _ => None,
    }
}

/// Check if a JSON-LD `@type` value indicates an article.
fn is_article_type(type_val: &serde_json::Value) -> bool {
    const ARTICLE_TYPES: &[&str] = &[
        "Article",
        "NewsArticle",
        "BlogPosting",
        "WebPage",
        "TechArticle",
        "ScholarlyArticle",
        "Report",
    ];

    match type_val {
        serde_json::Value::String(s) => ARTICLE_TYPES.iter().any(|t| s.eq_ignore_ascii_case(t)),
        serde_json::Value::Array(arr) => arr.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| ARTICLE_TYPES.iter().any(|t| s.eq_ignore_ascii_case(t)))
        }),
        _ => false,
    }
}

/// Extract author name from a JSON-LD object's `author` field.
fn extract_author_from_object(obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let author = obj.get("author")?;
    match author {
        serde_json::Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_owned())
            }
        }
        serde_json::Value::Object(a) => a
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        serde_json::Value::Array(arr) => arr.first().and_then(|a| match a {
            serde_json::Value::String(s) => {
                let s = s.trim();
                if s.is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                }
            }
            serde_json::Value::Object(o) => o
                .get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            _ => None,
        }),
        _ => None,
    }
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
        assert!(meta.favicon_url.is_none());
    }

    #[test]
    fn twitter_card_fallback() {
        let html = r#"
        <html>
        <head>
            <meta name="twitter:title" content="Twitter Title" />
            <meta name="twitter:description" content="Twitter desc" />
            <meta name="twitter:image" content="https://example.com/tw-image.jpg" />
            <meta name="twitter:creator" content="@janedoe" />
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.title.as_deref(), Some("Twitter Title"));
        assert_eq!(meta.description.as_deref(), Some("Twitter desc"));
        assert_eq!(
            meta.og_image.as_deref(),
            Some("https://example.com/tw-image.jpg")
        );
        assert_eq!(meta.author.as_deref(), Some("@janedoe"));
    }

    #[test]
    fn og_takes_priority_over_twitter_card() {
        let html = r#"
        <html>
        <head>
            <meta property="og:title" content="OG Title" />
            <meta name="twitter:title" content="Twitter Title" />
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.title.as_deref(), Some("OG Title"));
    }

    #[test]
    fn twitter_card_as_property() {
        let html = r#"
        <html>
        <head>
            <meta property="twitter:title" content="Twitter Via Property" />
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.title.as_deref(), Some("Twitter Via Property"));
    }

    #[test]
    fn extracts_favicon_icon() {
        let html = r#"
        <html>
        <head>
            <link rel="icon" href="/favicon.ico" />
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.favicon_url.as_deref(), Some("/favicon.ico"));
    }

    #[test]
    fn extracts_favicon_shortcut_icon() {
        let html = r#"
        <html>
        <head>
            <link rel="shortcut icon" href="https://example.com/icon.png" />
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(
            meta.favicon_url.as_deref(),
            Some("https://example.com/icon.png")
        );
    }

    #[test]
    fn extracts_favicon_apple_touch() {
        let html = r#"
        <html>
        <head>
            <link rel="apple-touch-icon" href="/apple-icon.png" />
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.favicon_url.as_deref(), Some("/apple-icon.png"));
    }

    #[test]
    fn jsonld_author_simple_object() {
        let html = r#"
        <html>
        <head>
            <script type="application/ld+json">
            {
                "@type": "Article",
                "author": { "name": "JSON-LD Author" }
            }
            </script>
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.author.as_deref(), Some("JSON-LD Author"));
    }

    #[test]
    fn jsonld_author_string() {
        let html = r#"
        <html>
        <head>
            <script type="application/ld+json">
            {
                "@type": "NewsArticle",
                "author": "Jane Reporter"
            }
            </script>
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.author.as_deref(), Some("Jane Reporter"));
    }

    #[test]
    fn jsonld_author_array() {
        let html = r#"
        <html>
        <head>
            <script type="application/ld+json">
            {
                "@type": "BlogPosting",
                "author": [
                    { "name": "First Author" },
                    { "name": "Second Author" }
                ]
            }
            </script>
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.author.as_deref(), Some("First Author"));
    }

    #[test]
    fn jsonld_author_in_graph() {
        let html = r#"
        <html>
        <head>
            <script type="application/ld+json">
            {
                "@graph": [
                    { "@type": "WebSite", "name": "Example" },
                    { "@type": "Article", "author": { "name": "Graph Author" } }
                ]
            }
            </script>
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.author.as_deref(), Some("Graph Author"));
    }

    #[test]
    fn meta_author_takes_priority_over_jsonld() {
        let html = r#"
        <html>
        <head>
            <meta name="author" content="Meta Author" />
            <script type="application/ld+json">
            { "@type": "Article", "author": "JSON-LD Author" }
            </script>
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html);
        assert_eq!(meta.author.as_deref(), Some("Meta Author"));
    }
}
