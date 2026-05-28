//! Kindle My Clippings.txt parser.
//!
//! Parses the standard `My Clippings.txt` file exported by Kindle devices.
//! Each clipping entry is separated by `==========` and contains the book
//! title/author, metadata (type, location, date), and the clipping content.

use time::OffsetDateTime;
use time::macros::format_description;

use crate::error::ImportError;

/// The type of a Kindle clipping entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindleClippingType {
    /// A highlighted passage.
    Highlight,
    /// A user-written note.
    Note,
    /// A bookmark (no content).
    Bookmark,
}

/// A single parsed entry from a Kindle `My Clippings.txt` file.
#[derive(Debug, Clone)]
pub struct KindleClipping {
    /// Book title as shown on the Kindle.
    pub book_title: String,
    /// Author name, if present in the title line.
    pub author: Option<String>,
    /// The type of clipping (highlight, note, or bookmark).
    pub clipping_type: KindleClippingType,
    /// Location string (e.g. "345-46" or "Page 23").
    pub location: Option<String>,
    /// When the clipping was added.
    pub added_at: Option<OffsetDateTime>,
    /// The clipping text content (empty for bookmarks).
    pub content: String,
}

/// Parse a Kindle `My Clippings.txt` file from raw bytes.
///
/// Splits the file on `==========` separators and extracts each clipping entry.
/// Tolerant of BOM markers, varying line endings, and locale-dependent date
/// formats. Entries that cannot be parsed are silently skipped.
///
/// # Errors
///
/// Returns `ImportError::Utf8` or `ImportError::Utf8Str` if the file is not
/// valid UTF-8.
pub fn parse_kindle_clippings(bytes: &[u8]) -> Result<Vec<KindleClipping>, ImportError> {
    let text = decode_utf8_bom(bytes)?;
    let mut clippings = Vec::new();

    for block in text.split("==========") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        if let Some(clipping) = parse_block(block) {
            clippings.push(clipping);
        }
    }

    Ok(clippings)
}

/// Decode bytes as UTF-8, stripping a BOM if present.
fn decode_utf8_bom(bytes: &[u8]) -> Result<&str, ImportError> {
    // UTF-8 BOM is EF BB BF
    let stripped = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    Ok(std::str::from_utf8(stripped)?)
}

/// Parse a single clipping block (between `==========` separators).
fn parse_block(block: &str) -> Option<KindleClipping> {
    let lines: Vec<&str> = block.lines().collect();
    if lines.len() < 2 {
        return None;
    }

    // Line 0: "Book Title (Author Name)" or just "Book Title"
    let (book_title, author) = parse_title_author(lines[0]);
    if book_title.is_empty() {
        return None;
    }

    // Line 1: "- Your Highlight on Location 345-46 | Added on Monday, May 27, 2024 10:21:26 PM"
    let meta_line = lines[1].trim();
    let (clipping_type, location, added_at) = parse_meta_line(meta_line);
    let clipping_type = clipping_type?;

    // Lines 2+: content (may be empty for bookmarks, may have a blank line first)
    let content = lines[2..].to_vec().join("\n").trim().to_owned();

    Some(KindleClipping {
        book_title,
        author,
        clipping_type,
        location,
        added_at,
        content,
    })
}

/// Parse the title/author line.
///
/// Format: `"Book Title (Author Name)"` or `"Book Title"`.
/// Splits on the last `(` to avoid breaking titles with parentheses.
fn parse_title_author(line: &str) -> (String, Option<String>) {
    let line = line.trim();
    // Some Kindle files have a Unicode BOM per-line or invisible chars
    let line = line.trim_start_matches('\u{feff}');

    if let Some(paren_pos) = line.rfind('(') {
        if line.ends_with(')') && paren_pos > 0 {
            let title = line[..paren_pos].trim().to_owned();
            let author = line[paren_pos + 1..line.len() - 1].trim().to_owned();
            if !author.is_empty() {
                return (title, Some(author));
            }
            return (title, None);
        }
    }
    (line.to_owned(), None)
}

/// Parse the metadata line to extract clipping type, location, and date.
///
/// Format: `"- Your Highlight on Location 345-46 | Added on Monday, May 27, 2024 10:21:26 PM"`
/// Or: `"- Your Highlight on Page 23 | Location 345-46 | Added on ..."`
fn parse_meta_line(
    line: &str,
) -> (
    Option<KindleClippingType>,
    Option<String>,
    Option<OffsetDateTime>,
) {
    // Strip leading "- " prefix
    let line = line.strip_prefix("- ").unwrap_or(line).trim();

    // Determine clipping type from the beginning of the line
    let clipping_type = if starts_with_ignore_case(line, "your highlight") {
        Some(KindleClippingType::Highlight)
    } else if starts_with_ignore_case(line, "your note") {
        Some(KindleClippingType::Note)
    } else if starts_with_ignore_case(line, "your bookmark") {
        Some(KindleClippingType::Bookmark)
    } else {
        // Try common non-English prefixes or just unknown
        None
    };

    // Split on " | " to get segments
    let segments: Vec<&str> = line.split(" | ").collect();

    let mut location = None;
    let mut added_at = None;

    for segment in &segments {
        let seg = segment.trim();
        if let Some(rest) = strip_prefix_ignore_case(seg, "added on ") {
            added_at = parse_kindle_date(rest);
        } else if let Some(loc) = extract_location(seg) {
            // Prefer "Location X" over "Page X" — a later segment may override.
            if location.is_none() || seg.to_lowercase().contains("location ") {
                location = Some(loc);
            }
        }
    }

    (clipping_type, location, added_at)
}

/// Extract location from a metadata segment like "Your Highlight on Location 345-46"
/// or "Your Highlight on Page 23 | Location 345-46".
fn extract_location(segment: &str) -> Option<String> {
    let lower = segment.to_lowercase();

    // Look for "location X" or "page X" in the segment
    if let Some(idx) = lower.find("location ") {
        let start = idx + "location ".len();
        let loc = segment[start..].trim();
        if !loc.is_empty() {
            return Some(loc.to_owned());
        }
    }
    if let Some(idx) = lower.find("page ") {
        let start = idx + "page ".len();
        let rest = segment[start..].trim();
        // Page number ends at next space or pipe
        let page = rest.split_whitespace().next().unwrap_or(rest);
        if !page.is_empty() {
            return Some(format!("Page {page}"));
        }
    }
    None
}

/// Parse a Kindle date string like "Monday, May 27, 2024 10:21:26 PM".
///
/// Tolerant of missing day-of-week or minor format variations.
fn parse_kindle_date(raw: &str) -> Option<OffsetDateTime> {
    let raw = raw.trim();

    // Strip the day-of-week prefix if present (e.g., "Monday, ")
    let date_str = raw.find(", ").map_or(raw, |comma_pos| {
        let before_comma = &raw[..comma_pos];
        if before_comma.chars().all(char::is_alphabetic) {
            raw[comma_pos + 2..].trim()
        } else {
            raw
        }
    });

    // Try "Month Day, Year HH:MM:SS AM/PM"
    let fmt_12h = format_description!(
        "[month repr:long] [day], [year] [hour repr:12]:[minute]:[second] [period case:upper]"
    );
    if let Ok(pdt) = time::PrimitiveDateTime::parse(date_str, &fmt_12h) {
        return Some(pdt.assume_utc());
    }

    // Try "Month Day, Year HH:MM:SS" (24-hour, no AM/PM)
    let fmt_24h =
        format_description!("[month repr:long] [day], [year] [hour repr:24]:[minute]:[second]");
    if let Ok(pdt) = time::PrimitiveDateTime::parse(date_str, &fmt_24h) {
        return Some(pdt.assume_utc());
    }

    // Try "Day Month Year HH:MM:SS" (UK/international format)
    let fmt_intl =
        format_description!("[day] [month repr:long] [year] [hour repr:24]:[minute]:[second]");
    if let Ok(pdt) = time::PrimitiveDateTime::parse(date_str, &fmt_intl) {
        return Some(pdt.assume_utc());
    }

    None
}

/// Case-insensitive `starts_with`.
fn starts_with_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Case-insensitive prefix strip.
fn strip_prefix_ignore_case<'a>(haystack: &'a str, prefix: &str) -> Option<&'a str> {
    if starts_with_ignore_case(haystack, prefix) {
        Some(&haystack[prefix.len()..])
    } else {
        None
    }
}

/// Generate a stable synthetic URL for a Kindle book (for dedup across imports).
///
/// Uses a simple hash of the normalized title and author.
#[must_use]
pub fn kindle_source_key(title: &str, author: Option<&str>) -> String {
    let norm_title = title.trim().to_lowercase();
    let norm_author = author.map_or(String::new(), |a| a.trim().to_lowercase());
    let mut hash = 0u64;
    for b in norm_title.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    hash = hash.wrapping_mul(31).wrapping_add(0xFF);
    for b in norm_author.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    format!("kindle://book/{hash:016x}")
}

/// Generate a stable synthetic URL for a Kindle highlight (for dedup).
///
/// Combines the source key with location and a hash of the quote text.
#[must_use]
pub fn kindle_highlight_key(
    title: &str,
    author: Option<&str>,
    location: Option<&str>,
    quote: &str,
) -> String {
    let source = kindle_source_key(title, author);
    let loc = location.unwrap_or("unknown");
    let norm_quote = quote.trim().to_lowercase();
    let mut hash = 0u64;
    for b in loc.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    for b in norm_quote.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    // Replace "kindle://book/" prefix with "kindle://highlight/"
    format!(
        "kindle://highlight/{}:{hash:016x}",
        &source["kindle://book/".len()..]
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SAMPLE_CLIPPINGS: &str = "\u{feff}The Great Gatsby (F. Scott Fitzgerald)
- Your Highlight on Page 23 | Location 345-346 | Added on Monday, May 27, 2024 10:21:26 PM

So we beat on, boats against the current, borne back ceaselessly into the past.
==========
The Great Gatsby (F. Scott Fitzgerald)
- Your Note on Location 351 | Added on Monday, May 27, 2024 10:23:11 PM

I love this final sentence.
==========
The Great Gatsby (F. Scott Fitzgerald)
- Your Bookmark on Page 24 | Location 356 | Added on Monday, May 27, 2024 10:25:05 PM

==========
Thinking, Fast and Slow (Daniel Kahneman)
- Your Highlight on Location 1234-1256 | Added on Tuesday, June 4, 2024 3:15:42 PM

Nothing in life is as important as you think it is, while you are thinking about it.
==========
";

    #[test]
    fn parse_basic_clippings() {
        let items = parse_kindle_clippings(SAMPLE_CLIPPINGS.as_bytes()).unwrap();
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn highlight_parsed_correctly() {
        let items = parse_kindle_clippings(SAMPLE_CLIPPINGS.as_bytes()).unwrap();
        let first = &items[0];
        assert_eq!(first.book_title, "The Great Gatsby");
        assert_eq!(first.author.as_deref(), Some("F. Scott Fitzgerald"));
        assert_eq!(first.clipping_type, KindleClippingType::Highlight);
        assert_eq!(first.location.as_deref(), Some("345-346"));
        assert!(first.added_at.is_some());
        assert!(first.content.contains("boats against the current"));
    }

    #[test]
    fn note_parsed_correctly() {
        let items = parse_kindle_clippings(SAMPLE_CLIPPINGS.as_bytes()).unwrap();
        let note = &items[1];
        assert_eq!(note.clipping_type, KindleClippingType::Note);
        assert_eq!(note.location.as_deref(), Some("351"));
        assert_eq!(note.content, "I love this final sentence.");
    }

    #[test]
    fn bookmark_parsed_correctly() {
        let items = parse_kindle_clippings(SAMPLE_CLIPPINGS.as_bytes()).unwrap();
        let bm = &items[2];
        assert_eq!(bm.clipping_type, KindleClippingType::Bookmark);
        assert!(bm.content.is_empty());
    }

    #[test]
    fn multiple_books_parsed() {
        let items = parse_kindle_clippings(SAMPLE_CLIPPINGS.as_bytes()).unwrap();
        let kahneman = &items[3];
        assert_eq!(kahneman.book_title, "Thinking, Fast and Slow");
        assert_eq!(kahneman.author.as_deref(), Some("Daniel Kahneman"));
        assert_eq!(kahneman.clipping_type, KindleClippingType::Highlight);
    }

    #[test]
    fn title_without_author() {
        let data = b"Just a Title\n- Your Highlight on Location 10 | Added on Monday, May 27, 2024 10:21:26 PM\n\nSome text.\n==========\n";
        let items = parse_kindle_clippings(data).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].book_title, "Just a Title");
        assert!(items[0].author.is_none());
    }

    #[test]
    fn empty_file() {
        let items = parse_kindle_clippings(b"").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn malformed_entries_skipped() {
        let data = b"==========\nJust one line\n==========\n\n==========\n";
        let items = parse_kindle_clippings(data).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn kindle_source_key_stable() {
        let k1 = kindle_source_key("The Great Gatsby", Some("F. Scott Fitzgerald"));
        let k2 = kindle_source_key("The Great Gatsby", Some("F. Scott Fitzgerald"));
        assert_eq!(k1, k2);
        assert!(k1.starts_with("kindle://book/"));
    }

    #[test]
    fn kindle_source_key_case_insensitive() {
        let k1 = kindle_source_key("The Great Gatsby", Some("F. Scott Fitzgerald"));
        let k2 = kindle_source_key("the great gatsby", Some("f. scott fitzgerald"));
        assert_eq!(k1, k2);
    }

    #[test]
    fn kindle_highlight_key_stable() {
        let k1 = kindle_highlight_key("Title", Some("Author"), Some("123"), "some text");
        let k2 = kindle_highlight_key("Title", Some("Author"), Some("123"), "some text");
        assert_eq!(k1, k2);
        assert!(k1.starts_with("kindle://highlight/"));
    }

    #[test]
    fn kindle_highlight_key_differs_by_location() {
        let k1 = kindle_highlight_key("Title", Some("Author"), Some("100"), "text");
        let k2 = kindle_highlight_key("Title", Some("Author"), Some("200"), "text");
        assert_ne!(k1, k2);
    }

    #[test]
    fn date_parsing_12h() {
        let dt = parse_kindle_date("Monday, May 27, 2024 10:21:26 PM");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), time::Month::May);
        assert_eq!(dt.day(), 27);
    }

    #[test]
    fn date_parsing_without_day_name() {
        let dt = parse_kindle_date("May 27, 2024 10:21:26 PM");
        assert!(dt.is_some());
    }

    #[test]
    fn date_parsing_invalid_returns_none() {
        let dt = parse_kindle_date("not a date");
        assert!(dt.is_none());
    }

    #[test]
    fn windows_line_endings() {
        let data = "The Great Gatsby (Author)\r\n- Your Highlight on Location 100 | Added on Monday, May 27, 2024 10:21:26 PM\r\n\r\nHighlight text.\r\n==========\r\n";
        let items = parse_kindle_clippings(data.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "Highlight text.");
    }
}
