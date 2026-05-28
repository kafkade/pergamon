//! PDF text-layer extraction via `lopdf`.
//!
//! Extracts embedded text from PDF documents. OCR is explicitly
//! not supported in this version (see ADR-004).

use crate::error::ExtractError;

/// Text extracted from a PDF document.
#[derive(Debug, Clone)]
pub struct ExtractedPdf {
    /// Extracted plain text content.
    pub text: String,
    /// Number of pages in the document.
    pub page_count: u32,
}

/// Extract text from a PDF byte stream.
///
/// Only extracts embedded text layers — scanned/image-only PDFs
/// will return an empty or minimal text result. OCR is deferred
/// to a future version.
///
/// # Errors
///
/// Returns [`ExtractError::Pdf`] if the PDF cannot be parsed.
pub fn extract_pdf_text(bytes: &[u8]) -> Result<ExtractedPdf, ExtractError> {
    let doc = lopdf::Document::load_mem(bytes).map_err(|e| ExtractError::Pdf(e.to_string()))?;

    #[allow(clippy::cast_possible_truncation)]
    let page_count = doc.get_pages().len() as u32;

    let mut text = String::new();
    for (page_num, _) in doc.get_pages() {
        if let Ok(page_text) = doc.extract_text(&[page_num]) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&page_text);
        }
    }

    Ok(ExtractedPdf { text, page_count })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_pdf_returns_error() {
        let result = extract_pdf_text(b"this is not a PDF");
        assert!(result.is_err());
        match result {
            Err(ExtractError::Pdf(_)) => {} // expected
            other => unreachable!("expected Pdf error, got {other:?}"),
        }
    }
}
