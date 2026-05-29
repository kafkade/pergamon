//! Manifest types for the Obsidian export bridge.
//!
//! The manifest is a JSON index file written alongside exported Markdown
//! notes. The Obsidian plugin reads it to populate the browse/search
//! modal without parsing every Markdown file on startup.

use serde::{Deserialize, Serialize};

/// Top-level manifest written to `.pergamon/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version for forward compatibility.
    pub schema_version: u32,
    /// pergamon version that produced this export.
    pub pergamon_version: String,
    /// ISO 8601 timestamp of the export.
    pub exported_at: String,
    /// Root folder name within the vault (e.g. "Pergamon").
    pub root_folder: String,
    /// Total number of exported items.
    pub item_count: usize,
    /// Index entries for every exported file.
    pub items: Vec<ManifestItem>,
}

/// A single entry in the manifest index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestItem {
    /// Pergamon UUID of the source content item.
    pub id: String,
    /// Item type: "highlight-source", "bookmark", "article".
    pub item_type: String,
    /// Display title for the browse modal.
    pub title: String,
    /// Original URL (if any).
    pub url: Option<String>,
    /// Author name (if known).
    pub author: Option<String>,
    /// Tag names.
    pub tags: Vec<String>,
    /// Number of highlights in this note.
    pub highlight_count: usize,
    /// Relative path from vault root to the exported `.md` file.
    pub file_path: String,
    /// ISO 8601 creation date.
    pub created_at: String,
    /// ISO 8601 last-update date.
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn manifest_serializes_to_json() {
        let manifest = Manifest {
            schema_version: 1,
            pergamon_version: "0.3.0".to_owned(),
            exported_at: "2026-05-28T12:00:00Z".to_owned(),
            root_folder: "Pergamon".to_owned(),
            item_count: 1,
            items: vec![ManifestItem {
                id: "abc-123".to_owned(),
                item_type: "highlight-source".to_owned(),
                title: "Test Article".to_owned(),
                url: Some("https://example.com".to_owned()),
                author: Some("Author".to_owned()),
                tags: vec!["rust".to_owned()],
                highlight_count: 3,
                file_path: "Pergamon/Highlights/test-article--abc12345.md".to_owned(),
                created_at: "2026-05-28".to_owned(),
                updated_at: "2026-05-28".to_owned(),
            }],
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"highlight_count\": 3"));
        assert!(json.contains("\"rust\""));
    }
}
