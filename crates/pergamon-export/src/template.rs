//! Configurable filename templates for exported files.
//!
//! Supports a fixed set of placeholders (`{title}`, `{date}`, `{id}`,
//! `{type}`) that are expanded at render time. Unknown placeholders are
//! rejected at parse time so callers get early feedback.

use std::fmt::Write;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::slug;

/// A validated filename template.
///
/// Templates contain literal text interleaved with placeholders from a
/// fixed set. The `.md` extension is appended automatically — do not
/// include it in the template.
///
/// # Supported placeholders
///
/// | Placeholder | Expands to |
/// |-------------|------------|
/// | `{title}`   | Slugified title (max 60 chars) |
/// | `{date}`    | `YYYY-MM-DD` creation date |
/// | `{id}`      | First 8 hex chars of the UUID |
/// | `{type}`    | Content type (`article`, `bookmark`, etc.) |
///
/// # Default
///
/// `{title}--{id}` — produces filenames like `my-article--a1b2c3d4.md`.
#[derive(Debug, Clone)]
pub struct SlugTemplate {
    segments: Vec<Segment>,
    has_id: bool,
}

/// A segment of a parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Title,
    Date,
    Id,
    Type,
}

/// Errors that can occur when parsing a slug template.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// An unknown placeholder was found.
    #[error("unknown placeholder: {{{0}}}")]
    UnknownPlaceholder(String),
    /// An unclosed brace was found.
    #[error("unclosed '{{' in template")]
    UnclosedBrace,
    /// The template is empty.
    #[error("template is empty")]
    Empty,
}

impl SlugTemplate {
    /// Parse and validate a template string.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError`] if the template contains unknown
    /// placeholders, unclosed braces, or is empty.
    pub fn parse(template: &str) -> Result<Self, TemplateError> {
        if template.is_empty() {
            return Err(TemplateError::Empty);
        }

        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut chars = template.chars();
        let mut has_id = false;

        while let Some(c) = chars.next() {
            if c == '{' {
                // Collect placeholder name.
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => name.push(ch),
                        None => return Err(TemplateError::UnclosedBrace),
                    }
                }

                // Flush preceding literal.
                if !literal.is_empty() {
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                }

                match name.as_str() {
                    "title" => segments.push(Segment::Title),
                    "date" => segments.push(Segment::Date),
                    "id" => {
                        has_id = true;
                        segments.push(Segment::Id);
                    }
                    "type" => segments.push(Segment::Type),
                    other => return Err(TemplateError::UnknownPlaceholder(other.to_owned())),
                }
            } else {
                literal.push(c);
            }
        }

        if !literal.is_empty() {
            segments.push(Segment::Literal(literal));
        }

        Ok(Self { segments, has_id })
    }

    /// Whether this template includes the `{id}` placeholder.
    ///
    /// Templates without `{id}` can produce filename collisions when
    /// multiple items share the same title and date.
    #[must_use]
    pub const fn has_id_placeholder(&self) -> bool {
        self.has_id
    }

    /// Render the template with concrete values.
    ///
    /// Returns the filename **without** the `.md` extension.
    #[must_use]
    pub fn render(
        &self,
        title: &str,
        id: Uuid,
        date: OffsetDateTime,
        content_type: &str,
    ) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                Segment::Literal(s) => out.push_str(s),
                Segment::Title => out.push_str(&slug::slugify(title, 60)),
                Segment::Date => {
                    let _ = write!(
                        out,
                        "{:04}-{:02}-{:02}",
                        date.year(),
                        u8::from(date.month()),
                        date.day(),
                    );
                }
                Segment::Id => out.push_str(&id.to_string()[..8]),
                Segment::Type => out.push_str(&slug::slugify(content_type, 30)),
            }
        }
        out
    }

    /// Render a full filename with `.md` extension.
    #[must_use]
    pub fn render_filename(
        &self,
        title: &str,
        id: Uuid,
        date: OffsetDateTime,
        content_type: &str,
    ) -> String {
        format!("{}.md", self.render(title, id, date, content_type))
    }
}

impl Default for SlugTemplate {
    fn default() -> Self {
        Self {
            segments: vec![
                Segment::Title,
                Segment::Literal("--".to_owned()),
                Segment::Id,
            ],
            has_id: true,
        }
    }
}

impl std::fmt::Display for SlugTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for seg in &self.segments {
            match seg {
                Segment::Literal(s) => write!(f, "{s}")?,
                Segment::Title => write!(f, "{{title}}")?,
                Segment::Date => write!(f, "{{date}}")?,
                Segment::Id => write!(f, "{{id}}")?,
                Segment::Type => write!(f, "{{type}}")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn test_id() -> Uuid {
        Uuid::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap()
    }

    fn test_date() -> OffsetDateTime {
        // 2026-03-18
        OffsetDateTime::from_unix_timestamp(1_773_811_200).unwrap()
    }

    #[test]
    fn default_template() {
        let tmpl = SlugTemplate::default();
        let name = tmpl.render_filename("My Article", test_id(), test_date(), "article");
        assert_eq!(name, "my-article--a1b2c3d4.md");
    }

    #[test]
    fn parse_default_pattern() {
        let tmpl = SlugTemplate::parse("{title}--{id}").unwrap();
        let name = tmpl.render_filename("Hello World", test_id(), test_date(), "article");
        assert_eq!(name, "hello-world--a1b2c3d4.md");
    }

    #[test]
    fn date_title_pattern() {
        let tmpl = SlugTemplate::parse("{date} - {title}").unwrap();
        let name = tmpl.render_filename("My Article", test_id(), test_date(), "article");
        assert_eq!(name, "2026-03-18 - my-article.md");
    }

    #[test]
    fn type_in_pattern() {
        let tmpl = SlugTemplate::parse("{type}/{title}--{id}").unwrap();
        let name = tmpl.render("Blog Post", test_id(), test_date(), "article");
        assert_eq!(name, "article/blog-post--a1b2c3d4");
    }

    #[test]
    fn all_placeholders() {
        let tmpl = SlugTemplate::parse("{date}_{type}_{title}_{id}").unwrap();
        let name = tmpl.render("Test", test_id(), test_date(), "bookmark");
        assert_eq!(name, "2026-03-18_bookmark_test_a1b2c3d4");
    }

    #[test]
    fn unknown_placeholder_rejected() {
        let err = SlugTemplate::parse("{title}--{unknown}").unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn unclosed_brace_rejected() {
        let err = SlugTemplate::parse("{title}--{id").unwrap_err();
        assert!(err.to_string().contains("unclosed"));
    }

    #[test]
    fn empty_rejected() {
        let err = SlugTemplate::parse("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn has_id_placeholder_true() {
        let tmpl = SlugTemplate::parse("{title}--{id}").unwrap();
        assert!(tmpl.has_id_placeholder());
    }

    #[test]
    fn has_id_placeholder_false() {
        let tmpl = SlugTemplate::parse("{date} - {title}").unwrap();
        assert!(!tmpl.has_id_placeholder());
    }

    #[test]
    fn display_round_trip() {
        let original = "{date} - {title}--{id}";
        let tmpl = SlugTemplate::parse(original).unwrap();
        assert_eq!(tmpl.to_string(), original);
    }

    #[test]
    fn literal_only_template() {
        let tmpl = SlugTemplate::parse("export-notes").unwrap();
        let name = tmpl.render_filename("Anything", test_id(), test_date(), "article");
        assert_eq!(name, "export-notes.md");
    }
}
