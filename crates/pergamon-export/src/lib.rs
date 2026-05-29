//! Export formatters for pergamon.
//!
//! This crate provides export functionality for writing pergamon data
//! to external formats: Obsidian vault notes (Markdown + YAML frontmatter),
//! and future formats (JSON, CSV, plain Markdown).
//!
//! The crate is intentionally I/O-capable — it reads and writes files —
//! but does **not** depend on `pergamon-storage`. Callers query the
//! database and pass structured data to the export functions.

pub mod frontmatter;
pub mod manifest;
pub mod obsidian;
pub mod slug;
