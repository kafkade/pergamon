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
