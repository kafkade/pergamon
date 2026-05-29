//! Filename-safe slug generation.
//!
//! Produces URL/filename-safe slugs from arbitrary Unicode strings.
//! Slugs are used as the human-readable portion of exported filenames
//! (e.g. `article-title--a1b2c3d4.md`).

/// Generate a filename-safe slug from an input string.
///
/// Rules:
/// - ASCII alphanumerics are kept, lowercased
/// - Spaces and non-alphanumeric characters become hyphens
/// - Consecutive hyphens are collapsed
/// - Leading and trailing hyphens are stripped
/// - Result is truncated to `max_len` characters (on a hyphen boundary if possible)
/// - Empty inputs produce `"untitled"`
#[must_use]
pub fn slugify(input: &str, max_len: usize) -> String {
    let slug: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    // Collapse consecutive hyphens.
    let mut collapsed = String::with_capacity(slug.len());
    let mut prev_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen {
                collapsed.push('-');
            }
            prev_hyphen = true;
        } else {
            collapsed.push(c);
            prev_hyphen = false;
        }
    }

    // Trim leading/trailing hyphens.
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        return "untitled".to_owned();
    }

    truncate_on_boundary(trimmed, max_len)
}

/// Truncate a slug to `max_len`, preferring to break on a hyphen boundary.
fn truncate_on_boundary(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_owned();
    }

    let candidate = &s[..max_len];

    // Try to find the last hyphen to break cleanly.
    if let Some(pos) = candidate.rfind('-') {
        // Only break on hyphen if we keep at least half the max length.
        if pos >= max_len / 2 {
            return candidate[..pos].to_owned();
        }
    }

    candidate.to_owned()
}

/// Format a stable filename: `{slug}--{short_id}.md`
///
/// The short ID is the first 8 characters of the UUID (hex), providing
/// enough uniqueness while keeping filenames readable.
#[must_use]
pub fn stable_filename(title: &str, id: uuid::Uuid) -> String {
    let slug = slugify(title, 60);
    let short_id = &id.to_string()[..8];
    format!("{slug}--{short_id}.md")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn basic_slugify() {
        assert_eq!(slugify("Hello World", 60), "hello-world");
    }

    #[test]
    fn special_characters() {
        assert_eq!(
            slugify("Rust: A Systems Language (2024)", 60),
            "rust-a-systems-language-2024"
        );
    }

    #[test]
    fn consecutive_special_chars() {
        assert_eq!(slugify("foo---bar___baz", 60), "foo-bar-baz");
    }

    #[test]
    fn leading_trailing_hyphens() {
        assert_eq!(slugify("---hello---", 60), "hello");
    }

    #[test]
    fn empty_input() {
        assert_eq!(slugify("", 60), "untitled");
    }

    #[test]
    fn all_special_chars() {
        assert_eq!(slugify("!!!@@@###", 60), "untitled");
    }

    #[test]
    fn truncation() {
        let long = "this-is-a-very-long-title-that-exceeds-the-maximum-length-allowed";
        let result = slugify(long, 30);
        assert!(result.len() <= 30);
    }

    #[test]
    fn truncation_on_hyphen_boundary() {
        let result = slugify("the quick brown fox jumps over", 20);
        // Should break on a hyphen boundary.
        assert!(!result.ends_with('-'));
        assert!(result.len() <= 20);
    }

    #[test]
    fn unicode_characters() {
        assert_eq!(slugify("café résumé naïve", 60), "caf-r-sum-na-ve");
    }

    #[test]
    fn stable_filename_format() {
        let id = uuid::Uuid::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap();
        let name = stable_filename("My Article Title", id);
        assert_eq!(name, "my-article-title--a1b2c3d4.md");
    }

    #[test]
    fn stable_filename_empty_title() {
        let id = uuid::Uuid::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap();
        let name = stable_filename("", id);
        assert_eq!(name, "untitled--a1b2c3d4.md");
    }

    #[test]
    fn windows_invalid_chars() {
        assert_eq!(
            slugify("file:name<with>invalid|chars", 60),
            "file-name-with-invalid-chars"
        );
    }
}
