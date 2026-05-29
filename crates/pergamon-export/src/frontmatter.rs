//! YAML frontmatter generation for Obsidian-compatible Markdown notes.
//!
//! Produces frontmatter blocks delimited by `---` with proper YAML
//! escaping. Values are always double-quoted to handle special
//! characters safely.

use std::fmt::Write as _;

/// A key-value pair in the frontmatter.
pub enum FrontmatterValue {
    /// A scalar string value.
    Str(String),
    /// A list of string values (rendered as YAML flow sequence).
    List(Vec<String>),
    /// An integer value.
    Int(i64),
}

/// Render a frontmatter block from key-value pairs.
///
/// Keys are emitted in insertion order. String values are double-quoted
/// with internal `"` and `\` escaped. Lists use YAML flow-sequence
/// syntax: `[a, b, c]`.
#[must_use]
pub fn render_frontmatter(pairs: &[(&str, FrontmatterValue)]) -> String {
    let mut out = String::from("---\n");
    for (key, value) in pairs {
        match value {
            FrontmatterValue::Str(s) => {
                let _ = writeln!(out, "{key}: \"{}\"", yaml_escape(s));
            }
            FrontmatterValue::List(items) => {
                if items.is_empty() {
                    let _ = writeln!(out, "{key}: []");
                } else {
                    let formatted: Vec<String> = items
                        .iter()
                        .map(|s| format!("\"{}\"", yaml_escape(s)))
                        .collect();
                    let _ = writeln!(out, "{key}: [{}]", formatted.join(", "));
                }
            }
            FrontmatterValue::Int(n) => {
                let _ = writeln!(out, "{key}: {n}");
            }
        }
    }
    out.push_str("---\n");
    out
}

/// Escape a string value for use inside double-quoted YAML.
///
/// Escapes `\`, `"`, and newlines.
fn yaml_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_frontmatter() {
        let pairs = vec![
            ("title", FrontmatterValue::Str("Hello World".to_owned())),
            ("count", FrontmatterValue::Int(42)),
        ];
        let result = render_frontmatter(&pairs);
        assert_eq!(result, "---\ntitle: \"Hello World\"\ncount: 42\n---\n");
    }

    #[test]
    fn tag_list() {
        let pairs = vec![(
            "tags",
            FrontmatterValue::List(vec!["rust".to_owned(), "programming".to_owned()]),
        )];
        let result = render_frontmatter(&pairs);
        assert_eq!(result, "---\ntags: [\"rust\", \"programming\"]\n---\n");
    }

    #[test]
    fn empty_list() {
        let pairs = vec![("tags", FrontmatterValue::List(vec![]))];
        let result = render_frontmatter(&pairs);
        assert_eq!(result, "---\ntags: []\n---\n");
    }

    #[test]
    fn escapes_quotes() {
        let pairs = vec![(
            "title",
            FrontmatterValue::Str("She said \"hello\"".to_owned()),
        )];
        let result = render_frontmatter(&pairs);
        assert!(result.contains(r#"title: "She said \"hello\"""#));
    }

    #[test]
    fn escapes_backslash() {
        let pairs = vec![("path", FrontmatterValue::Str("C:\\Users\\file".to_owned()))];
        let result = render_frontmatter(&pairs);
        assert!(result.contains(r#"path: "C:\\Users\\file""#));
    }

    #[test]
    fn escapes_newlines() {
        let pairs = vec![("note", FrontmatterValue::Str("line1\nline2".to_owned()))];
        let result = render_frontmatter(&pairs);
        assert!(result.contains(r#"note: "line1\nline2""#));
    }

    #[test]
    fn yaml_special_chars_in_values() {
        let pairs = vec![(
            "title",
            FrontmatterValue::Str("key: value # comment".to_owned()),
        )];
        let result = render_frontmatter(&pairs);
        // Quoting protects colons, hashes, etc.
        assert!(result.contains(r#"title: "key: value # comment""#));
    }

    #[test]
    fn boolean_like_values_are_quoted() {
        let pairs = vec![("value", FrontmatterValue::Str("true".to_owned()))];
        let result = render_frontmatter(&pairs);
        // Quoted, so YAML parsers won't interpret as boolean.
        assert!(result.contains(r#"value: "true""#));
    }
}
