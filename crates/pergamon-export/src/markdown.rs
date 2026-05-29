//! General-purpose Markdown export with YAML frontmatter.
//!
//! Exports pergamon content items as individual Markdown files with rich
//! YAML frontmatter. Unlike the Obsidian exporter ([`super::obsidian`])
//! which groups highlights by source, this exporter produces one file per
//! content item with a stable, documented schema.
//!
//! # Schema version
//!
//! The current schema version is **1**. Frontmatter includes an
//! `export_schema` field for forward compatibility.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use pergamon_core::model::{ContentItem, HighlightMeta, Note};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::frontmatter::{FrontmatterValue, render_frontmatter};
use crate::template::SlugTemplate;

/// Current schema version for the Markdown export format.
pub const SCHEMA_VERSION: u32 = 1;

// ======================================================================
// Configuration
// ======================================================================

/// How tags appear in exported Markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagFormat {
    /// Tags only in YAML frontmatter (default).
    #[default]
    YamlOnly,
    /// `#tag-name` in the document body.
    Hashtag,
    /// Both frontmatter and body hashtags.
    Both,
}

/// Configuration for a Markdown export.
#[derive(Debug, Clone)]
pub struct MarkdownExportConfig {
    /// Filename template (default: `{title}--{id}`).
    pub slug_template: SlugTemplate,
    /// Whether to generate wikilinks for cross-references.
    pub backlinks: bool,
    /// How tags are rendered.
    pub tag_format: TagFormat,
    /// The pergamon version string for the frontmatter.
    pub pergamon_version: String,
}

impl Default for MarkdownExportConfig {
    fn default() -> Self {
        Self {
            slug_template: SlugTemplate::default(),
            backlinks: false,
            tag_format: TagFormat::default(),
            pergamon_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

// ======================================================================
// Export item
// ======================================================================

/// A content item bundled with all related data for export.
///
/// This is the generic export data type. Callers build these from
/// database queries regardless of content type.
#[derive(Debug, Clone)]
pub struct ExportItem {
    /// The content item.
    pub item: ContentItem,
    /// Tag names.
    pub tags: Vec<String>,
    /// Highlights on this item (each with its own `HighlightMeta`).
    pub highlights: Vec<(ContentItem, HighlightMeta)>,
    /// Free-form notes attached to this item.
    pub notes: Vec<Note>,
}

/// A rendered Markdown file ready to be written.
#[derive(Debug, Clone)]
pub struct RenderedFile {
    /// Path relative to the output directory.
    pub relative_path: PathBuf,
    /// Full Markdown content (frontmatter + body).
    pub content: String,
    /// Pergamon UUID of the primary item.
    pub pergamon_id: Uuid,
}

/// A cross-reference target for backlink generation.
#[derive(Debug, Clone)]
struct BacklinkTarget {
    /// Display title.
    title: String,
    /// Rendered filename (without `.md`).
    stem: String,
}

/// Result of executing a Markdown export.
#[derive(Debug, Clone, Default)]
pub struct MarkdownExportResult {
    /// Number of files written.
    pub written: usize,
    /// Total files in the plan.
    pub total: usize,
}

/// Errors during Markdown export.
#[derive(Debug, thiserror::Error)]
pub enum MarkdownExportError {
    /// I/O error writing files.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ======================================================================
// Planning
// ======================================================================

/// Plan a Markdown export without touching the filesystem.
///
/// Computes filenames, renders frontmatter and body for each item,
/// and resolves backlinks using an internal cross-reference index.
#[must_use]
pub fn plan_markdown_export(
    config: &MarkdownExportConfig,
    items: &[ExportItem],
) -> Vec<RenderedFile> {
    // Build backlink index: UUID → (title, filename stem).
    let backlink_index: HashMap<Uuid, BacklinkTarget> = if config.backlinks {
        items
            .iter()
            .map(|ei| {
                let stem = config.slug_template.render(
                    &ei.item.title,
                    ei.item.id,
                    ei.item.created_at,
                    &ei.item.content_type.to_string(),
                );
                (
                    ei.item.id,
                    BacklinkTarget {
                        title: ei.item.title.clone(),
                        stem,
                    },
                )
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Track filenames for collision detection.
    let mut seen_filenames: HashMap<String, usize> = HashMap::new();

    let mut files = Vec::with_capacity(items.len());

    for ei in items {
        let mut filename = config.slug_template.render_filename(
            &ei.item.title,
            ei.item.id,
            ei.item.created_at,
            &ei.item.content_type.to_string(),
        );

        // Collision resolution: append `--{id}` if filename already used
        // and the template doesn't include `{id}`.
        if !config.slug_template.has_id_placeholder() {
            let count = seen_filenames.entry(filename.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                // Strip .md, append --{id}.md.
                let stem = filename.trim_end_matches(".md");
                filename = format!("{}--{}.md", stem, &ei.item.id.to_string()[..8]);
            }
        }
        seen_filenames.entry(filename.clone()).or_insert(0);

        let content = render_item(config, ei, &backlink_index);

        files.push(RenderedFile {
            relative_path: PathBuf::from(&filename),
            content,
            pergamon_id: ei.item.id,
        });
    }

    files
}

/// Execute a planned Markdown export: write files to the output directory.
///
/// Creates the output directory if needed.
///
/// # Errors
///
/// Returns [`MarkdownExportError::Io`] if any file operation fails.
pub fn execute_markdown_export(
    files: &[RenderedFile],
    output_dir: &Path,
) -> Result<MarkdownExportResult, MarkdownExportError> {
    std::fs::create_dir_all(output_dir)?;

    let mut result = MarkdownExportResult {
        total: files.len(),
        ..MarkdownExportResult::default()
    };

    for file in files {
        let full_path = output_dir.join(&file.relative_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, &file.content)?;
        result.written += 1;
    }

    Ok(result)
}

// ======================================================================
// Rendering
// ======================================================================

/// Render YAML frontmatter pairs for an export item.
fn build_frontmatter_pairs(ei: &ExportItem) -> Vec<(&'static str, FrontmatterValue)> {
    let mut pairs: Vec<(&str, FrontmatterValue)> = vec![
        ("pergamon_id", FrontmatterValue::Str(ei.item.id.to_string())),
        (
            "export_schema",
            FrontmatterValue::Int(i64::from(SCHEMA_VERSION)),
        ),
        ("title", FrontmatterValue::Str(ei.item.title.clone())),
        (
            "content_type",
            FrontmatterValue::Str(ei.item.content_type.to_string()),
        ),
    ];

    if let Some(ref author) = ei.item.author {
        pairs.push(("author", FrontmatterValue::Str(author.clone())));
    }
    if let Some(ref url) = ei.item.url {
        pairs.push(("url", FrontmatterValue::Str(url.clone())));
    }
    if !ei.tags.is_empty() {
        let normalized: Vec<String> = ei
            .tags
            .iter()
            .map(|t| t.replace(' ', "-").to_lowercase())
            .collect();
        pairs.push(("tags", FrontmatterValue::List(normalized)));
    }
    pairs.push(("status", FrontmatterValue::Str(ei.item.status.to_string())));
    if !ei.highlights.is_empty() {
        pairs.push((
            "highlight_count",
            FrontmatterValue::Int(i64::try_from(ei.highlights.len()).unwrap_or(0)),
        ));
    }
    pairs.push((
        "created",
        FrontmatterValue::Str(format_date(ei.item.created_at)),
    ));
    pairs.push((
        "updated",
        FrontmatterValue::Str(format_date(ei.item.updated_at)),
    ));

    pairs
}

/// Render a single export item as Markdown with YAML frontmatter.
fn render_item(
    config: &MarkdownExportConfig,
    ei: &ExportItem,
    backlink_index: &HashMap<Uuid, BacklinkTarget>,
) -> String {
    let pairs = build_frontmatter_pairs(ei);
    let mut out = render_frontmatter(&pairs);

    // Title heading.
    let _ = writeln!(out, "\n# {}\n", ei.item.title);

    // Metadata block.
    if let Some(ref author) = ei.item.author {
        let _ = writeln!(out, "**Author:** {author}");
    }
    if let Some(ref url) = ei.item.url {
        let domain = extract_domain(url);
        let _ = writeln!(out, "**URL:** [{domain}]({url})");
    }
    let _ = writeln!(out, "**Type:** {}", ei.item.content_type);
    let _ = writeln!(out, "**Status:** {}\n", ei.item.status);

    // Tags as hashtags in body (if configured).
    if matches!(config.tag_format, TagFormat::Hashtag | TagFormat::Both) && !ei.tags.is_empty() {
        let hashtags: Vec<String> = ei
            .tags
            .iter()
            .map(|t| format!("#{}", t.replace(' ', "-").to_lowercase()))
            .collect();
        let _ = writeln!(out, "{}\n", hashtags.join(" "));
    }

    // Excerpt.
    if let Some(ref excerpt) = ei.item.excerpt {
        let _ = writeln!(out, "> {excerpt}\n");
    }

    // Highlights section.
    if !ei.highlights.is_empty() {
        let _ = writeln!(out, "## Highlights\n");
        for (_item, meta) in &ei.highlights {
            // Multi-line highlight: prefix every line with `> `.
            for line in meta.quote_text.lines() {
                let _ = writeln!(out, "> {line}");
            }
            out.push('\n');

            if let Some(ref note) = meta.note {
                let _ = writeln!(out, "*{note}*\n");
            }
            if let Some(ref color) = meta.color {
                let _ = writeln!(out, "Color: {color}\n");
            }
            out.push_str("---\n\n");
        }
    }

    // Notes section.
    if !ei.notes.is_empty() {
        let _ = writeln!(out, "## Notes\n");
        for note in &ei.notes {
            let date = format_date(note.created_at);
            let _ = writeln!(out, "- {} *({})*\n", note.body, date);
        }
    }

    // Related items (backlinks).
    if config.backlinks {
        let mut related = Vec::new();

        // From highlights: link to the source item.
        for (_item, meta) in &ei.highlights {
            if let Some(source_id) = meta.source_item_id {
                if source_id != ei.item.id {
                    if let Some(target) = backlink_index.get(&source_id) {
                        related.push(target);
                    }
                }
            }
        }

        // Deduplicate by stem.
        related.sort_by(|a, b| a.stem.cmp(&b.stem));
        related.dedup_by(|a, b| a.stem == b.stem);

        if !related.is_empty() {
            let _ = writeln!(out, "## Related\n");
            for target in &related {
                let _ = writeln!(out, "- [[{}|{}]]", target.stem, target.title);
            }
            out.push('\n');
        }
    }

    out
}

// ======================================================================
// Helpers
// ======================================================================

/// Extract the domain from a URL for display.
fn extract_domain(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

/// Format an `OffsetDateTime` as `YYYY-MM-DD`.
fn format_date(dt: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        dt.year(),
        u8::from(dt.month()),
        dt.day()
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use pergamon_core::content_type::ContentType;
    use pergamon_core::status::DocumentStatus;

    fn make_item(title: &str) -> ContentItem {
        ContentItem {
            id: Uuid::new_v4(),
            url: Some("https://example.com/article".to_owned()),
            title: title.to_owned(),
            author: Some("Author Name".to_owned()),
            content_type: ContentType::Article,
            status: DocumentStatus::Archived,
            content_text: None,
            excerpt: Some("A short excerpt.".to_owned()),
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            read_at: None,
        }
    }

    fn make_export_item(title: &str) -> ExportItem {
        ExportItem {
            item: make_item(title),
            tags: vec!["rust".to_owned(), "programming".to_owned()],
            highlights: vec![],
            notes: vec![],
        }
    }

    #[test]
    fn basic_markdown_export() {
        let config = MarkdownExportConfig::default();
        let items = vec![make_export_item("Test Article")];
        let files = plan_markdown_export(&config, &items);

        assert_eq!(files.len(), 1);
        let file = &files[0];

        // Check frontmatter.
        assert!(file.content.contains("export_schema: 1"));
        assert!(file.content.contains("title: \"Test Article\""));
        assert!(file.content.contains("content_type: \"article\""));
        assert!(file.content.contains("author: \"Author Name\""));
        assert!(file.content.contains("tags: [\"rust\", \"programming\"]"));
        assert!(file.content.contains("created: \"1970-01-01\""));

        // Check body.
        assert!(file.content.contains("# Test Article"));
        assert!(file.content.contains("**Author:** Author Name"));
        assert!(file.content.contains("**URL:** [example.com]"));
    }

    #[test]
    fn markdown_with_highlights() {
        let config = MarkdownExportConfig::default();
        let mut ei = make_export_item("Article With Highlights");

        let source_id = ei.item.id;
        let hl_item = ContentItem {
            id: Uuid::new_v4(),
            content_type: ContentType::Highlight,
            status: DocumentStatus::Inbox,
            title: String::new(),
            url: None,
            author: None,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            read_at: None,
        };
        let hl_meta = HighlightMeta {
            content_item_id: hl_item.id,
            source_item_id: Some(source_id),
            quote_text: "This is important.".to_owned(),
            note: Some("Remember this.".to_owned()),
            position_start: None,
            position_end: None,
            color: Some("yellow".to_owned()),
        };
        ei.highlights.push((hl_item, hl_meta));

        let files = plan_markdown_export(&config, &[ei]);
        let content = &files[0].content;

        assert!(content.contains("## Highlights"));
        assert!(content.contains("> This is important."));
        assert!(content.contains("*Remember this.*"));
        assert!(content.contains("Color: yellow"));
        assert!(content.contains("highlight_count: 1"));
    }

    #[test]
    fn multiline_highlight_prefixed() {
        let config = MarkdownExportConfig::default();
        let mut ei = make_export_item("Multi Line");

        let hl_item = ContentItem {
            id: Uuid::new_v4(),
            content_type: ContentType::Highlight,
            status: DocumentStatus::Inbox,
            title: String::new(),
            url: None,
            author: None,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            read_at: None,
        };
        let hl_meta = HighlightMeta {
            content_item_id: hl_item.id,
            source_item_id: None,
            quote_text: "Line one.\nLine two.\nLine three.".to_owned(),
            note: None,
            position_start: None,
            position_end: None,
            color: None,
        };
        ei.highlights.push((hl_item, hl_meta));

        let files = plan_markdown_export(&config, &[ei]);
        let content = &files[0].content;

        assert!(content.contains("> Line one.\n> Line two.\n> Line three."));
    }

    #[test]
    fn markdown_with_notes() {
        let config = MarkdownExportConfig::default();
        let mut ei = make_export_item("Notes Article");

        ei.notes.push(Note {
            id: Uuid::new_v4(),
            content_item_id: ei.item.id,
            body: "An important observation.".to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        });

        let files = plan_markdown_export(&config, &[ei]);
        let content = &files[0].content;

        assert!(content.contains("## Notes"));
        assert!(content.contains("An important observation."));
    }

    #[test]
    fn hashtag_format() {
        let config = MarkdownExportConfig {
            tag_format: TagFormat::Hashtag,
            ..MarkdownExportConfig::default()
        };
        let items = vec![make_export_item("Tagged")];
        let files = plan_markdown_export(&config, &items);

        assert!(files[0].content.contains("#rust #programming"));
    }

    #[test]
    fn both_tag_format() {
        let config = MarkdownExportConfig {
            tag_format: TagFormat::Both,
            ..MarkdownExportConfig::default()
        };
        let items = vec![make_export_item("Both Tags")];
        let files = plan_markdown_export(&config, &items);
        let content = &files[0].content;

        // Frontmatter tags.
        assert!(content.contains("tags: [\"rust\", \"programming\"]"));
        // Body hashtags.
        assert!(content.contains("#rust #programming"));
    }

    #[test]
    fn custom_slug_template() {
        let config = MarkdownExportConfig {
            slug_template: SlugTemplate::parse("{date} - {title}").unwrap(),
            ..MarkdownExportConfig::default()
        };
        let items = vec![make_export_item("Custom Name")];
        let files = plan_markdown_export(&config, &items);

        let path = files[0].relative_path.to_string_lossy();
        assert!(path.contains("1970-01-01 - custom-name.md"));
    }

    #[test]
    fn collision_detection_without_id() {
        let config = MarkdownExportConfig {
            slug_template: SlugTemplate::parse("{title}").unwrap(),
            ..MarkdownExportConfig::default()
        };

        // Two items with the same title.
        let items = vec![make_export_item("Duplicate"), make_export_item("Duplicate")];
        let files = plan_markdown_export(&config, &items);

        assert_eq!(files.len(), 2);
        // First gets plain name, second gets --{id} suffix.
        assert_eq!(files[0].relative_path.to_string_lossy(), "duplicate.md");
        let second = files[1].relative_path.to_string_lossy();
        assert!(second.starts_with("duplicate--"), "got: {second}");
        assert!(second.ends_with(".md"));
    }

    #[test]
    fn backlinks_generated() {
        let config = MarkdownExportConfig {
            backlinks: true,
            ..MarkdownExportConfig::default()
        };

        let source_item = make_item("Source Article");
        let source_id = source_item.id;

        // Highlight that references the source.
        let mut highlight_export = make_export_item("My Highlight");
        let hl_item = ContentItem {
            id: Uuid::new_v4(),
            content_type: ContentType::Highlight,
            status: DocumentStatus::Inbox,
            title: String::new(),
            url: None,
            author: None,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            read_at: None,
        };
        let hl_meta = HighlightMeta {
            content_item_id: hl_item.id,
            source_item_id: Some(source_id),
            quote_text: "Important quote.".to_owned(),
            note: None,
            position_start: None,
            position_end: None,
            color: None,
        };
        highlight_export.highlights.push((hl_item, hl_meta));

        let source_export = ExportItem {
            item: source_item,
            tags: vec![],
            highlights: vec![],
            notes: vec![],
        };

        let files = plan_markdown_export(&config, &[source_export, highlight_export]);

        // The highlight file should have a Related section linking to the source.
        let hl_content = &files[1].content;
        assert!(hl_content.contains("## Related"));
        assert!(hl_content.contains("[[source-article--"));
        assert!(hl_content.contains("|Source Article]]"));
    }

    #[test]
    fn excerpt_in_output() {
        let config = MarkdownExportConfig::default();
        let items = vec![make_export_item("With Excerpt")];
        let files = plan_markdown_export(&config, &items);

        assert!(files[0].content.contains("> A short excerpt."));
    }

    #[test]
    fn execute_writes_files() {
        let config = MarkdownExportConfig::default();
        let items = vec![make_export_item("Write Test")];
        let files = plan_markdown_export(&config, &items);

        let tmp = std::env::temp_dir().join(format!("pergamon-md-test-{}", Uuid::new_v4()));
        let result = execute_markdown_export(&files, &tmp).unwrap();

        assert_eq!(result.written, 1);
        assert_eq!(result.total, 1);

        let written = std::fs::read_to_string(tmp.join(&files[0].relative_path)).unwrap();
        assert!(written.contains("# Write Test"));

        // Clean up.
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
