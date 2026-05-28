//! # pergamon-extract
//!
//! Article extraction, HTML sanitization, and PDF text extraction.
//!
//! This crate handles:
//! - Readability-based article extraction from raw HTML
//! - HTML sanitization via `ammonia` to produce safe, normalized content
//! - Metadata extraction from Open Graph / meta tags
//! - PDF text-layer extraction for local document import
//!
//! This crate performs no I/O. Callers are responsible for fetching
//! content and passing raw bytes or strings.

pub mod article;
pub mod canonical;
pub mod error;
pub mod metadata;
pub mod pdf;

pub use article::{ExtractedArticle, extract_article, extract_article_from_html};
pub use canonical::canonicalize_url;
pub use error::ExtractError;
pub use metadata::{Metadata, extract_metadata};
pub use pdf::{ExtractedPdf, extract_pdf_text};

/// Resolve a possibly-relative favicon URL against a base page URL.
///
/// Returns `None` if the favicon href is absent or the base URL is
/// unparseable. Falls back to the raw href if resolution fails.
#[must_use]
pub fn resolve_favicon_url(favicon_href: &str, base_url: &str) -> Option<String> {
    if favicon_href.starts_with("http://") || favicon_href.starts_with("https://") {
        return Some(favicon_href.to_owned());
    }
    url::Url::parse(base_url)
        .ok()
        .and_then(|base| base.join(favicon_href).ok())
        .map(|u| u.to_string())
}
