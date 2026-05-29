//! Obsidian vault exporter.
//!
//! Exports pergamon highlights, articles, and bookmarks as Markdown
//! notes with YAML frontmatter into an Obsidian vault folder. Generates
//! a `.pergamon/manifest.json` index for the companion Obsidian plugin.
//!
//! # Architecture
//!
//! Export is split into two phases:
//! 1. **Planning** ([`plan_export`]) — computes the list of files to write
//!    and their content, without touching the filesystem. This is testable
//!    in isolation.
//! 2. **Execution** ([`execute_export`]) — writes the planned files to disk,
//!    atomically replacing the manifest last.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use pergamon_core::model::{ContentItem, HighlightMeta, Note};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::frontmatter::{FrontmatterValue, render_frontmatter};
use crate::manifest::{Manifest, ManifestItem};
use crate::slug;

/// Configuration for an Obsidian export.
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Root folder name within the vault (default: `"Pergamon"`).
    pub folder_name: String,
    /// The pergamon version string for the manifest.
    pub pergamon_version: String,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            folder_name: "Pergamon".to_owned(),
            pergamon_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// A source document bundled with its highlights, notes, and tags.
///
/// Callers construct these from database queries and pass them to
/// [`plan_export`]. The exporter renders each bundle as a single
/// Markdown note containing all highlights from the source.
#[derive(Debug, Clone)]
pub struct SourceBundle {
    /// The source content item (article, feed item, bookmark).
    pub source: ContentItem,
    /// Tag names for this source.
    pub tags: Vec<String>,
    /// Highlights from this source, as `(ContentItem, HighlightMeta)` pairs.
    pub highlights: Vec<(ContentItem, HighlightMeta)>,
    /// Free-form notes on this source.
    pub notes: Vec<Note>,
}

/// A standalone bookmark without highlights, exported as its own note.
#[derive(Debug, Clone)]
pub struct BookmarkBundle {
    /// The bookmark content item.
    pub item: ContentItem,
    /// Tag names.
    pub tags: Vec<String>,
    /// Description from bookmark metadata.
    pub description: Option<String>,
}

/// A file to be written during export.
#[derive(Debug, Clone)]
pub struct ExportFile {
    /// Path relative to the vault root.
    pub relative_path: PathBuf,
    /// Full Markdown content (frontmatter + body).
    pub content: String,
    /// Pergamon UUID of the primary item.
    pub pergamon_id: Uuid,
}

/// Result of executing an export plan.
#[derive(Debug, Clone, Default)]
pub struct ExportResult {
    /// Number of files created or updated.
    pub written: usize,
    /// Number of files in the manifest.
    pub total: usize,
}

/// The complete export plan: files to write and manifest to generate.
#[derive(Debug, Clone)]
pub struct ExportPlan {
    /// Files to write (Markdown notes).
    pub files: Vec<ExportFile>,
    /// The manifest JSON for the plugin.
    pub manifest: Manifest,
}

/// Errors during export.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// I/O error writing files.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

// ======================================================================
// Planning
// ======================================================================

/// Plan an export without touching the filesystem.
///
/// Takes source bundles (documents with highlights) and bookmark bundles,
/// and computes the Markdown content and file paths for each.
#[must_use]
pub fn plan_export(
    config: &ExportConfig,
    sources: &[SourceBundle],
    bookmarks: &[BookmarkBundle],
) -> ExportPlan {
    let now = format_rfc3339_now();
    let mut files = Vec::new();
    let mut manifest_items = Vec::new();

    // Export source documents with highlights.
    for bundle in sources {
        let filename = slug::stable_filename(&bundle.source.title, bundle.source.id);
        let relative_path = PathBuf::from(&config.folder_name)
            .join("Highlights")
            .join(&filename);

        let content = render_source_note(bundle);

        manifest_items.push(ManifestItem {
            id: bundle.source.id.to_string(),
            item_type: "highlight-source".to_owned(),
            title: bundle.source.title.clone(),
            url: bundle.source.url.clone(),
            author: bundle.source.author.clone(),
            tags: bundle.tags.clone(),
            highlight_count: bundle.highlights.len(),
            file_path: path_to_posix(&relative_path),
            created_at: format_date(bundle.source.created_at),
            updated_at: format_date(bundle.source.updated_at),
        });

        files.push(ExportFile {
            relative_path,
            content,
            pergamon_id: bundle.source.id,
        });
    }

    // Export bookmarks.
    for bundle in bookmarks {
        let filename = slug::stable_filename(&bundle.item.title, bundle.item.id);
        let relative_path = PathBuf::from(&config.folder_name)
            .join("Bookmarks")
            .join(&filename);

        let content = render_bookmark_note(bundle);

        manifest_items.push(ManifestItem {
            id: bundle.item.id.to_string(),
            item_type: "bookmark".to_owned(),
            title: bundle.item.title.clone(),
            url: bundle.item.url.clone(),
            author: bundle.item.author.clone(),
            tags: bundle.tags.clone(),
            highlight_count: 0,
            file_path: path_to_posix(&relative_path),
            created_at: format_date(bundle.item.created_at),
            updated_at: format_date(bundle.item.updated_at),
        });

        files.push(ExportFile {
            relative_path,
            content,
            pergamon_id: bundle.item.id,
        });
    }

    let manifest = Manifest {
        schema_version: 1,
        pergamon_version: config.pergamon_version.clone(),
        exported_at: now,
        root_folder: config.folder_name.clone(),
        item_count: files.len(),
        items: manifest_items,
    };

    ExportPlan { files, manifest }
}

// ======================================================================
// Execution
// ======================================================================

/// Execute an export plan: write files and manifest to the vault.
///
/// Creates directories as needed. Writes all Markdown files first,
/// then writes the manifest last for atomicity.
///
/// # Errors
///
/// Returns [`ExportError::Io`] if any file or directory operation fails.
pub fn execute_export(plan: &ExportPlan, vault_path: &Path) -> Result<ExportResult, ExportError> {
    let mut result = ExportResult {
        total: plan.files.len(),
        ..ExportResult::default()
    };

    // Write each Markdown file.
    for file in &plan.files {
        let full_path = vault_path.join(&file.relative_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, &file.content)?;
        result.written += 1;
    }

    // Write manifest last (atomic-ish: write to temp, then rename).
    let manifest_dir = vault_path
        .join(&plan.manifest.root_folder)
        .join(".pergamon");
    std::fs::create_dir_all(&manifest_dir)?;

    let manifest_json = serde_json::to_string_pretty(&plan.manifest)?;
    let tmp_path = manifest_dir.join("manifest.json.tmp");
    let final_path = manifest_dir.join("manifest.json");
    std::fs::write(&tmp_path, &manifest_json)?;
    std::fs::rename(&tmp_path, &final_path)?;

    Ok(result)
}

// ======================================================================
// Rendering
// ======================================================================

/// Render a source document note with all its highlights.
fn render_source_note(bundle: &SourceBundle) -> String {
    let mut pairs: Vec<(&str, FrontmatterValue)> = vec![
        (
            "pergamon_id",
            FrontmatterValue::Str(bundle.source.id.to_string()),
        ),
        (
            "pergamon_type",
            FrontmatterValue::Str(bundle.source.content_type.to_string()),
        ),
        ("title", FrontmatterValue::Str(bundle.source.title.clone())),
    ];

    if let Some(ref author) = bundle.source.author {
        pairs.push(("author", FrontmatterValue::Str(author.clone())));
    }
    if let Some(ref url) = bundle.source.url {
        pairs.push(("url", FrontmatterValue::Str(url.clone())));
    }
    if !bundle.tags.is_empty() {
        pairs.push(("tags", FrontmatterValue::List(obsidian_tags(&bundle.tags))));
        pairs.push(("pergamon_tags", FrontmatterValue::List(bundle.tags.clone())));
    }
    pairs.push((
        "status",
        FrontmatterValue::Str(bundle.source.status.to_string()),
    ));
    pairs.push((
        "highlight_count",
        FrontmatterValue::Int(i64::try_from(bundle.highlights.len()).unwrap_or(0)),
    ));
    pairs.push((
        "created",
        FrontmatterValue::Str(format_date(bundle.source.created_at)),
    ));
    pairs.push((
        "updated",
        FrontmatterValue::Str(format_date(bundle.source.updated_at)),
    ));

    let mut out = render_frontmatter(&pairs);

    // Title heading.
    let _ = writeln!(out, "\n# {}\n", bundle.source.title);

    // Metadata block.
    if let Some(ref author) = bundle.source.author {
        let _ = writeln!(out, "**Author:** {author}");
    }
    if let Some(ref url) = bundle.source.url {
        let domain = extract_domain(url);
        let _ = writeln!(out, "**URL:** [{domain}]({url})");
    }
    let _ = writeln!(out, "**Status:** {}\n", bundle.source.status);

    // Highlights section.
    if !bundle.highlights.is_empty() {
        let _ = writeln!(out, "## Highlights\n");
        for (_item, meta) in &bundle.highlights {
            let _ = writeln!(out, "> {}\n", meta.quote_text);
            if let Some(ref note) = meta.note {
                let _ = writeln!(out, "*Note: {note}*\n");
            }
            out.push_str("---\n\n");
        }
    }

    // Notes section.
    if !bundle.notes.is_empty() {
        let _ = writeln!(out, "## Notes\n");
        for note in &bundle.notes {
            let date = format_date(note.created_at);
            let _ = writeln!(out, "- {} *({})*\n", note.body, date);
        }
    }

    out
}

/// Render a bookmark note.
fn render_bookmark_note(bundle: &BookmarkBundle) -> String {
    let mut pairs: Vec<(&str, FrontmatterValue)> = vec![
        (
            "pergamon_id",
            FrontmatterValue::Str(bundle.item.id.to_string()),
        ),
        (
            "pergamon_type",
            FrontmatterValue::Str("bookmark".to_owned()),
        ),
        ("title", FrontmatterValue::Str(bundle.item.title.clone())),
    ];

    if let Some(ref author) = bundle.item.author {
        pairs.push(("author", FrontmatterValue::Str(author.clone())));
    }
    if let Some(ref url) = bundle.item.url {
        pairs.push(("url", FrontmatterValue::Str(url.clone())));
    }
    if !bundle.tags.is_empty() {
        pairs.push(("tags", FrontmatterValue::List(obsidian_tags(&bundle.tags))));
        pairs.push(("pergamon_tags", FrontmatterValue::List(bundle.tags.clone())));
    }
    pairs.push((
        "status",
        FrontmatterValue::Str(bundle.item.status.to_string()),
    ));
    pairs.push((
        "created",
        FrontmatterValue::Str(format_date(bundle.item.created_at)),
    ));
    pairs.push((
        "updated",
        FrontmatterValue::Str(format_date(bundle.item.updated_at)),
    ));

    let mut out = render_frontmatter(&pairs);

    let _ = writeln!(out, "\n# {}\n", bundle.item.title);

    if let Some(ref url) = bundle.item.url {
        let domain = extract_domain(url);
        let _ = writeln!(out, "**URL:** [{domain}]({url})");
    }
    let _ = writeln!(out, "**Status:** {}\n", bundle.item.status);

    if let Some(ref desc) = bundle.description {
        let _ = writeln!(out, "> {desc}\n");
    }

    if let Some(ref excerpt) = bundle.item.excerpt {
        if bundle.description.is_none() {
            let _ = writeln!(out, "> {excerpt}\n");
        }
    }

    out
}

// ======================================================================
// Helpers
// ======================================================================

/// Normalize tag names for Obsidian compatibility.
///
/// Obsidian tags cannot contain spaces. We replace spaces with hyphens
/// and lowercase everything.
fn obsidian_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|t| {
            t.chars()
                .map(|c| {
                    if c == ' ' {
                        '-'
                    } else {
                        c.to_ascii_lowercase()
                    }
                })
                .collect()
        })
        .collect()
}

/// Extract the domain from a URL for display (e.g. `example.com`).
fn extract_domain(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

/// Format an `OffsetDateTime` as an ISO 8601 date string (`YYYY-MM-DD`).
fn format_date(dt: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        dt.year(),
        u8::from(dt.month()),
        dt.day()
    )
}

/// Format the current time as an RFC 3339 string.
fn format_rfc3339_now() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
    )
}

/// Convert a `Path` to a POSIX-style string (forward slashes).
///
/// Obsidian uses forward slashes regardless of OS.
fn path_to_posix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Group highlights by their `source_item_id`.
///
/// Returns a map from source item ID to the list of highlights from
/// that source. Highlights without a source are grouped under a
/// synthetic "orphan" key.
#[must_use]
pub fn group_highlights_by_source(
    highlights: &[(ContentItem, HighlightMeta)],
) -> HashMap<Option<Uuid>, Vec<(ContentItem, HighlightMeta)>> {
    let mut grouped: HashMap<Option<Uuid>, Vec<(ContentItem, HighlightMeta)>> = HashMap::new();
    for (item, meta) in highlights {
        grouped
            .entry(meta.source_item_id)
            .or_default()
            .push((item.clone(), meta.clone()));
    }
    grouped
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
            excerpt: Some("An excerpt".to_owned()),
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn make_highlight(source_id: Uuid, quote: &str) -> (ContentItem, HighlightMeta) {
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: Some("https://example.com/article".to_owned()),
            title: quote.to_owned(),
            author: None,
            content_type: ContentType::Highlight,
            status: DocumentStatus::Archived,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        let meta = HighlightMeta {
            content_item_id: item.id,
            source_item_id: Some(source_id),
            quote_text: quote.to_owned(),
            note: Some("A note".to_owned()),
            position_start: None,
            position_end: None,
            color: None,
        };
        (item, meta)
    }

    #[test]
    fn plan_export_generates_files() {
        let source = make_item("Test Article");
        let source_id = source.id;
        let h1 = make_highlight(source_id, "Important insight");
        let h2 = make_highlight(source_id, "Another insight");

        let bundles = vec![SourceBundle {
            source,
            tags: vec!["rust".to_owned(), "testing".to_owned()],
            highlights: vec![h1, h2],
            notes: vec![],
        }];

        let config = ExportConfig {
            folder_name: "Pergamon".to_owned(),
            pergamon_version: "0.3.0".to_owned(),
        };

        let plan = plan_export(&config, &bundles, &[]);

        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.manifest.items.len(), 1);
        assert_eq!(plan.manifest.schema_version, 1);
        assert_eq!(plan.manifest.item_count, 1);

        let file = &plan.files[0];
        assert!(file.content.contains("pergamon_id:"));
        assert!(file.content.contains("## Highlights"));
        assert!(file.content.contains("> Important insight"));
        assert!(file.content.contains("> Another insight"));
        assert!(file.content.contains("*Note: A note*"));
    }

    #[test]
    fn plan_export_includes_bookmarks() {
        let bookmark = BookmarkBundle {
            item: make_item("Useful Link"),
            tags: vec!["reference".to_owned()],
            description: Some("A useful resource".to_owned()),
        };

        let config = ExportConfig::default();
        let plan = plan_export(&config, &[], &[bookmark]);

        assert_eq!(plan.files.len(), 1);

        let file = &plan.files[0];
        assert!(file.content.contains("pergamon_type: \"bookmark\""));
        assert!(file.content.contains("> A useful resource"));
        assert!(file.relative_path.to_string_lossy().contains("Bookmarks"));
    }

    #[test]
    fn source_note_contains_frontmatter() {
        let source = make_item("My Article");
        let bundle = SourceBundle {
            source,
            tags: vec!["rust".to_owned()],
            highlights: vec![],
            notes: vec![],
        };

        let content = render_source_note(&bundle);

        assert!(content.starts_with("---\n"));
        assert!(content.contains("pergamon_id:"));
        assert!(content.contains("pergamon_type: \"article\""));
        assert!(content.contains("title: \"My Article\""));
        assert!(content.contains("author: \"Author Name\""));
        assert!(content.contains("tags: [\"rust\"]"));
        assert!(content.contains("status: \"archived\""));
    }

    #[test]
    fn obsidian_tag_normalization() {
        let tags = vec!["Machine Learning".to_owned(), "Rust Lang".to_owned()];
        let normalized = obsidian_tags(&tags);
        assert_eq!(normalized, vec!["machine-learning", "rust-lang"]);
    }

    #[test]
    fn extract_domain_basic() {
        assert_eq!(extract_domain("https://example.com/path"), "example.com");
        assert_eq!(
            extract_domain("http://blog.example.com/post"),
            "blog.example.com"
        );
        assert_eq!(extract_domain("not-a-url"), "not-a-url");
    }

    #[test]
    fn path_to_posix_converts_backslashes() {
        let p = PathBuf::from("Pergamon\\Highlights\\file.md");
        assert_eq!(path_to_posix(&p), "Pergamon/Highlights/file.md");
    }

    #[test]
    fn group_highlights_groups_by_source() {
        let source1 = Uuid::new_v4();
        let source2 = Uuid::new_v4();

        let h1 = make_highlight(source1, "Quote 1");
        let h2 = make_highlight(source1, "Quote 2");
        let h3 = make_highlight(source2, "Quote 3");

        let all = vec![h1, h2, h3];
        let grouped = group_highlights_by_source(&all);

        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[&Some(source1)].len(), 2);
        assert_eq!(grouped[&Some(source2)].len(), 1);
    }

    #[test]
    fn execute_export_writes_files() {
        let source = make_item("Test Export");
        let bundle = SourceBundle {
            source,
            tags: vec![],
            highlights: vec![],
            notes: vec![],
        };

        let config = ExportConfig {
            folder_name: "Pergamon".to_owned(),
            pergamon_version: "0.3.0".to_owned(),
        };

        let plan = plan_export(&config, &[bundle], &[]);

        let tmp_dir = std::env::temp_dir().join(format!("pergamon-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let result = execute_export(&plan, &tmp_dir).unwrap();
        assert_eq!(result.written, 1);

        // Verify the file exists.
        let md_path = tmp_dir.join(&plan.files[0].relative_path);
        assert!(md_path.exists());

        // Verify the manifest exists.
        let manifest_path = tmp_dir
            .join("Pergamon")
            .join(".pergamon")
            .join("manifest.json");
        assert!(manifest_path.exists());

        // Clean up.
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn manifest_item_paths_use_forward_slashes() {
        let source = make_item("Slash Test");
        let bundle = SourceBundle {
            source,
            tags: vec![],
            highlights: vec![],
            notes: vec![],
        };

        let config = ExportConfig::default();
        let plan = plan_export(&config, &[bundle], &[]);

        for item in &plan.manifest.items {
            assert!(
                !item.file_path.contains('\\'),
                "path should use forward slashes"
            );
        }
    }
}
