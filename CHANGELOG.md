<!-- markdownlint-disable MD024 -->
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `highlight add` command: create highlights from any content item with optional `--note`, `--color`, and `--tag` flags
- `highlight list` command: list highlights with `--source`, `--tag`, `--since`, `--before`, `--limit`, and `--format` filters
- `highlight show` command: display full highlight details including source item, tags, and attached notes
- `highlight export` command: export highlights as Markdown or JSON with optional `--source` filter and `--output` file
- `note add` command: attach free-form notes to any content item
- `note list` command: list notes for a specific item or across all items with JSON output support
- `note edit` command: update an existing note's text
- `note delete` command: remove a note by ID
- TUI highlight capture: press `h` in reader view to create a highlight with a text input overlay
- Highlights are searchable via full-text search
- Auto-position detection for highlights: byte offsets are recorded when quote text uniquely matches the source
- Notes table with foreign key cascade deletion (V7 migration)
- Backup and restore now includes notes

## [0.2.0] - 2026-05-28

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
- OPML import: parse OPML files and create feed subscriptions with folder hierarchy (`import opml`)
- OPML export: generate OPML from subscribed feeds grouped by folder (`export opml`)
- Feed folder management: organize feeds into folders with `feed move` and `feed list --tree`
- Dry-run mode for OPML import to preview changes without modifying the database
- Idempotent re-import: existing subscriptions are detected by URL and folders reused by name
- TUI triage workflow: filter items by status, feed, or folder with keyboard-first navigation
- Quick status filters: `1`–`5` for inbox/later/reading/reference/archived, `0` for all, `Tab` to cycle
- Feed/folder picker overlay in the TUI (press `f` for feeds, `F` for folders)
- Bulk mark-as-read action with confirmation dialog (`R` key)
- Open current item in the default browser from the TUI (`o` key)
- Triage keybindings available in both list and reader views (`r`/`s`/`a`/`d`/`l`)
- Jump to top/bottom navigation (`g`/`G` or Home/End)
- Status-colored item rows and unread count in the TUI status bar
- URL display in the article reader header
- Filtered content item queries (`ContentItemFilter`) in the storage layer
- URL canonicalization for deduplication: strips tracking parameters, normalizes scheme/host/port, sorts query params (`pergamon-extract`)
- Duplicate detection for `pergamon save`: deduplicates against the canonical post-redirect URL
- `--tag` / `-t` flag for `pergamon save` to tag items on capture (repeatable)
- `--bookmark` flag for `pergamon save` to save as bookmark without article extraction
- Pipe support for `pergamon save`: read URL from stdin (`echo "https://..." | pergamon save`)
- Duplicate saves still apply new tags to the existing item
- `get_or_create_tag` storage method for race-safe tag creation by name
- V4 migration: partial unique index on `content_items.url` and case-insensitive unique index on `tags.name`
- `pergamon search <query>` command: full-text search across all content (title, author, body, tags)
- Search faceted filters: `--type`, `--tag`, `--status`, `--source`, `--since`, `--before`
- `--source` filter accepts feed title substring (case-insensitive) or UUID
- JSON output format for search results (`--format json`)
- Search results show BM25-ranked hits with snippet context
- TUI search: press `/` in list or reader view to search all content
- Search input bar with live typing, Enter to submit, Esc to cancel
- Help overlay updated with `/` search keybinding
- Full backup export: `pergamon export backup -o file.zip` creates a ZIP archive with all tables as JSON files plus a schema manifest
- Backup restore: `pergamon import backup file.zip` restores a full backup into an empty database with transactional safety
- Backup format validation: schema version check, manifest verification, non-empty database rejection
- `pergamon config` command: display current configuration with file path and load status
- Configuration file support: TOML config at platform-standard config directory with sensible defaults
- `pergamon completions <shell>` command: generate shell completions for bash, zsh, fish, and PowerShell
- Bulk listing methods in storage layer for backup export (all content items, collections, extension metadata, junction tables)
- `schema_version()` and `is_empty()` database introspection methods
- `pergamon collection` commands: `create`, `list` (flat and `--tree`), `rename`, `move` (with `--parent` or `--root`), `delete`, `add`, `remove`, `show`
- `pergamon tag` commands: `add`, `remove`, `list`, `rename`, `delete`, `show`
- `pergamon bulk` commands: `tag`, `move`, `archive`, `delete` with `--status`/`--type` filters and `--yes` confirmation skip
- Collections and tags can be referenced by name or UUID in all commands
- Nested collection hierarchy with cycle detection on moves
- "Unsorted" filter: `--uncollected` flag to find items not in any collection
- Bulk operations use transactions for atomicity and require confirmation before executing
- `pergamon import raindrop <file>` command: import bookmarks from a Raindrop.io CSV export with tags, collections, and provenance tracking
- `pergamon import pocket <file>` command: import bookmarks from a Pocket HTML export with tags and timestamps
- Dry-run mode for Raindrop and Pocket imports (`--dry-run`) to preview changes
- Idempotent re-import for Raindrop and Pocket: existing items get tags and collections updated
- URL canonicalization applied to all imported URLs for deduplication
- Import summary report showing created and existing (updated) item counts
- Metadata enrichment for saved URLs: Twitter Card fallback, favicon extraction, JSON-LD author parsing, and `og:site_name` support
- `pergamon save` now stores enriched `BookmarkMeta` (OG image, favicon, site name) for all saved URLs
- Re-saving a URL as `--bookmark` upserts metadata without creating a duplicate
- `pergamon doctor dupes` command: scan for duplicate URLs using canonical URL matching with confidence levels (exact vs. canonical)
- `pergamon doctor merge <keep> <discard>` command: safely merge two duplicate items — transfers tags and collections, preserves extension metadata, backdates `created_at`, and deletes the discarded item
- `pergamon doctor links` command: check link health by probing saved URLs — detects dead links (4xx), server errors (5xx), redirect chains, and connection failures
- `--stale <days>` flag for `doctor links` to only check URLs not verified in the last N days
- Link health results stored in database for incremental re-checking
