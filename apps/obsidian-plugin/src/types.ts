/**
 * Type definitions matching pergamon's export manifest schema.
 *
 * These mirror the Rust types in `crates/pergamon-export/src/manifest.rs`.
 */

/** Top-level manifest written by `pergamon export obsidian`. */
export interface PergamonManifest {
  /** Schema version for forward compatibility. */
  schema_version: number;
  /** pergamon version that produced this export. */
  pergamon_version: string;
  /** ISO 8601 timestamp of the export. */
  exported_at: string;
  /** Root folder name within the vault (e.g. "Pergamon"). */
  root_folder: string;
  /** Total number of exported items. */
  item_count: number;
  /** Index entries for every exported file. */
  items: ManifestItem[];
}

/** A single entry in the manifest index. */
export interface ManifestItem {
  /** Pergamon UUID of the source content item. */
  id: string;
  /** Item type: "highlight-source", "bookmark", "article". */
  item_type: string;
  /** Display title for the browse modal. */
  title: string;
  /** Original URL (if any). */
  url: string | null;
  /** Author name (if known). */
  author: string | null;
  /** Tag names. */
  tags: string[];
  /** Number of highlights in this note. */
  highlight_count: number;
  /** Relative path from vault root to the exported `.md` file. */
  file_path: string;
  /** ISO 8601 creation date. */
  created_at: string;
  /** ISO 8601 last-update date. */
  updated_at: string;
}
