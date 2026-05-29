//! Export formatters for pergamon.
//!
//! This crate provides export functionality for writing pergamon data
//! to external formats:
//!
//! - **Markdown** ([`markdown`]) — general-purpose Markdown export with
//!   YAML frontmatter, configurable filenames, and optional wikilink
//!   backlinks.
//! - **JSON** ([`json`]) — versioned JSON export with stable DTOs
//!   decoupled from the internal data model.
//! - **Obsidian** ([`obsidian`]) — Obsidian vault sync with highlight
//!   grouping and plugin manifest.
//!
//! The crate is intentionally I/O-capable — it reads and writes files —
//! but does **not** depend on `pergamon-storage`. Callers query the
//! database and pass structured data to the export functions.

pub mod frontmatter;
pub mod json;
pub mod manifest;
pub mod markdown;
pub mod obsidian;
pub mod slug;
pub mod template;
