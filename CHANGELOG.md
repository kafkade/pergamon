<!-- markdownlint-disable MD024 -->
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Repository scaffolding: GitHub templates, CI workflow, copilot instructions, contribution guide, and licensing (Apache-2.0)
- Architecture Decision Records (`docs/adr/ADR-001` through `ADR-010`)
- Product roadmap (`docs/roadmap.md`)
- Cargo workspace with five crates: `pergamon-core`, `pergamon-storage`, `pergamon-feed`, `pergamon-extract`, `pergamon-cli`
- CLI binary with `--info` flag (`pergamon --info`)
- Workspace-wide lint configuration (forbid unsafe, deny unwrap/expect/panic, clippy pedantic + nursery)
- Rust CI pipeline: check, test (Linux/macOS/Windows), clippy, fmt
- Unified content model: domain types for content items, feeds, tags, collections, highlights, and bookmarks (`pergamon-core`)
- SQLite schema with FTS5 full-text search, extension tables for type-specific metadata, and automatic `updated_at` triggers (`pergamon-storage`)
- CRUD operations for all content entities with filtered listing and full-text search
- Custom embedded migration runner for schema versioning
- Feed subscription commands: `feed add`, `feed list`, `feed refresh`, `feed remove`, and `sync`
- RSS/Atom/JSON Feed parsing via feed-rs with normalization to pergamon domain types
- Conditional GET support with ETag and Last-Modified headers for efficient feed polling
- Feed health tracking: error count, last error message, and last successful fetch timestamp
- Duplicate entry detection using GUID with URL fallback during feed ingestion
- Article extraction pipeline using readability algorithm with ammonia HTML sanitization (`pergamon-extract`)
- Metadata extraction from Open Graph, Twitter Card, and standard meta tags
- PDF text-layer extraction via lopdf
- `save <url>` command: fetch a web page, extract article content, and store as an inbox item
- `read` command: TUI inbox and article reader powered by ratatui with vim-style keybindings
- TUI keybindings for triage: `r` read, `l` later, `s` star, `a` archive, `d` discard
- Help overlay in the TUI (press `?` to toggle)
- Pagination support (limit/offset) for content item listing
- Status update and count queries for content items in storage layer
