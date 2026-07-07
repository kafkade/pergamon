<!-- markdownlint-disable MD024 -->

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `pergamon-crypto` crate: an Apache-2.0, client-side end-to-end-encryption library implementing the ADR-024 key scheme for sync (#125). Provides the account key hierarchy (Account Root Key → per-epoch account content keys → per-event and convergent per-blob keys), authenticated encryption of event bodies and blobs (XChaCha20-Poly1305) with the ADR-022 envelope header bound as associated data, `entity_ref` blinding (HMAC-SHA-256), device keypairs and signed device records (X25519 + Ed25519), an in-house sealed box for enrollment bundles, short-authentication-string (SAS) verification, trust/revocation attestations, passphrase- and recovery-code-based account recovery (Argon2id), and epoch rotation that re-wraps the new content key to the remaining devices. Pure derivations with a CSPRNG only for key/nonce generation; no I/O or networking
- Convergent blob encryption: identical plaintext yields identical ciphertext under a content-derived key scoped to the account content key, so the blind server can deduplicate encrypted blobs without learning anything about them
- Known-answer test vectors for the key hierarchy, `entity_ref` blinding, and convergent blob encryption, proving the derivations are deterministic and stable
- `pergamon-sync-server` opaque relay endpoints for multi-device onboarding artifacts (#125): device records (`PUT`/`GET /v1/devices/{account}/{device}`, list via `GET /v1/devices/{account}`), wrapped key bundles (`POST`/`GET /v1/wraps/{account}/{device}`), trust/revocation attestations (`POST`/`GET /v1/attestations/{account}`), and the account recovery blob (`PUT`/`GET /v1/recovery/{account}`). The server stores and relays these as opaque bytes only — it never decodes or interprets them — preserving the blind-relay invariant; puts are idempotent (upsert or append-with-dedup)
- `pergamon device-key` CLI command (#125): `init` generates and stores this device's sync keypairs (bootstrapping a new Account Root Key for the first device on an account), and `show` reports the stored `device_id` and whether an account root key is present. Keys are kept in the OS keychain (macOS Keychain, Linux Secret Service, Windows Credential Manager) by default, or in an Argon2id-encrypted key file for headless hosts (`--key-file` with `PERGAMON_KEY_PASSPHRASE`). Device secrets never leave the machine
- Sync server HTTP API: `GET /health`; `POST /v1/events` to push a batch of encrypted event envelopes (idempotent on client-supplied `change_id`, with upload-before-commit enforcement); `GET /v1/events` for cursor-based incremental pull ordered by a monotonic `server_seq`; `POST /v1/blobs/probe` to discover which ciphertext blobs are missing; and `PUT`/`GET /v1/blobs/{account}/{ct_hash}` to upload and download opaque, content-addressed (SHA-256-verified) blobs
- `pergamon-sync-server` binary with configurable host, port, and database path via CLI flags or environment variables (`PERGAMON_SYNC_HOST`, `PERGAMON_SYNC_PORT`, `PERGAMON_SYNC_DB`), and graceful shutdown on SIGINT/SIGTERM or Ctrl+C
- Optional end-to-end-encrypted multi-device sync (#126): pergamon can now push and pull your library to a self-hosted sync server while keeping everything encrypted on your own devices. Local edits are tracked and queued as they happen, then synced on demand; the server only ever stores opaque ciphertext and blinded identifiers, so it never sees your titles, notes, tags, URLs, or content. Sync is always opt-in and client-initiated — the local database stays the source of truth
- `pergamon sync-remote` CLI command (#126): `enable` links this device to a sync server and account, `push` uploads pending local changes, `pull` fetches and applies remote changes, `sync` does both, `status` shows the pending change count and sync cursor, and `conflicts` lists unresolved edit conflicts. Saving a URL now records a tracked change automatically, so it is included in the next sync
- Automatic conflict handling for synced edits (#126): concurrent changes from different devices converge deterministically — last-writer-wins for most fields, set-union for tags and collections, and a preserved conflict copy when two devices edit the same long-form text at once, so no edit is silently lost. Deletions win over concurrent edits
- `pergamon-sync` crate: an Apache-2.0 client sync engine implementing the ADR-022 wire protocol, ADR-023 conflict resolution, and ADR-024 encryption glue, with a pluggable transport (HTTP against the sync server, or in-memory for tests)
- Spaced-repetition review state now merges correctly across devices (#127): when you review the same card on two devices before syncing, both reviews are kept and the card's schedule is recomputed from the full review history, so due counts stay accurate and no review is dropped or double-counted
- Concurrent edits to highlights and notes are resolved by type (#127): editing a note or highlight on two devices at once preserves both versions (the losing edit is kept as a conflict copy), and deleting an annotation on one device while another device edits it no longer silently discards that edit — the unseen edit is surfaced as a conflict instead of being lost

## [0.7.0] - 2026-07-02

### Added

- iOS share extension for URL and text capture (#119): share a link and/or selected text from any app's share sheet straight into pergamon. The extension stages each capture instantly and offline — no network, extraction, or database work happens while you share — and the app finalizes staged captures the next time it opens or comes to the foreground, canonicalizing and de-duplicating the URL, saving it as a bookmark in the inbox, and attaching any shared selection as a highlight. Finalization is crash-safe and idempotent, so a capture is never lost or duplicated
- UniFFI surface additions for iOS share capture (#119): new `ShareCapture` and `ShareIngestOutcome` records and a `ShareContentKind` enum, plus `Library.ingest_share_capture`, which runs the shared canonicalize → dedupe → create/enrich bookmark → attach highlight pipeline so share-sheet captures land consistently with `save`
- iOS local-first offline database and backup import/export (#118): the iPhone app now stores everything in an on-device SQLite database — the same storage engine as the CLI — instead of an in-memory demo corpus, so saved items, highlights, tags, collections, and review cards persist across launches and work fully offline (the demo corpus is seeded only on first launch). Adds a Settings screen (gear in the Inbox) with backup **Export** (via the system share sheet) and **Restore** (via the file picker) using pergamon's portable archive format, so backups round-trip between the CLI and iOS. Restoring replaces the on-device library and refreshes every view. If the database cannot be opened, the app falls back to the in-memory seed so it still launches
- UniFFI surface additions for iOS storage/backup (#118): `Library` gains an `open(path)` constructor that opens and migrates the on-device SQLite store, plus `export_backup`, `restore_backup`, and `storage_info`; new `BackupSummary` and `StorageInfo` records. The CLI and iOS now share a single canonical backup implementation (`pergamon-storage::backup`), so an archive written by one client restores in the other
- iOS highlights capture and spaced-repetition review (#117): capture highlights from the reader via a composer sheet (quote + optional note), with a highlights section in the item detail for editing notes and deleting; a review queue driven by the core FSRS-5 scheduler with reveal + Again/Hard/Good/Easy grade buttons that write review logs and reschedule cards; and a due-count badge on the Review tab. Capturing a highlight auto-creates a review card (New, due now). Works offline and schedules consistently with the CLI review state
- UniFFI surface additions for iOS review (#117): new `Highlight`, `ReviewCardView`, and `ReviewSummary` records and `ReviewGrade`/`ReviewState` enums; and `Library` gains `highlights`, `add_highlight`, `set_highlight_note`, `delete_highlight`, `due_cards`, `grade_card`, and `review_summary`. Grading drives the real `pergamon_core::fsrs::Scheduler` (default FSRS-5 params), mirroring the CLI grade math for parity
- iOS search, bookmarks, and organization (#116): faceted search screen (content type, status, tag, source, and a published-within date preset) with active-facet chips; collection and tag browsing folded into the Saved tab via a scope picker (All / Status / Collections / Tags), including a nested collection tree; and tag/collection assignment from the item detail (assign existing entries or create new ones inline)
- UniFFI surface additions for iOS organization (#116): new `Tag`, `Collection`, and `SearchFacets` records; `ContentItem` now carries `tags` and `collection_ids`; and `Library` gains `search_filtered`, `tags`, `collections`, `items_with_tag`, `items_in_collection`, `add_tag`/`remove_tag`, `create_collection`, and `add_to_collection`/`remove_from_collection`. `search_filtered` matches title/author/excerpt/URL/content AND-combined with the facet set for parity with the CLI/web search
- Admin diagnostics view for pergamon-server: authenticated `/admin` dashboard covering feed health, extraction status, import history, system statistics, broken links, and a content-rules monitor (#72)
- Optional HTTP Basic auth for the admin subtree via `--admin-user`/`--admin-password` flags or `PERGAMON_ADMIN_USER`/`PERGAMON_ADMIN_PASSWORD` env vars (routes stay open with a startup warning when unset)
- Diagnostics logging: new `import_log` and `extraction_log` tables (migration V12) with CLI import/save and server feed-sync instrumentation
- Admin feed-sync actions: sync a single feed or all feeds from the dashboard (no-JS form fallbacks)
- Web UI (server-rendered HTML) for pergamon-server: inbox/library list and article reader views (#68)
- Inbox view: feed/folder sidebar with unread counts, status/type/tag/source filters, sort by date/title/source, and pagination
- Article reader view: extracted content, metadata header, original-link, inline status triage, and tag add/remove
- HTMX-powered partial updates for triage, tagging, filtering, and bulk actions, with full-page fallbacks when JavaScript is unavailable (progressive enhancement)
- Bulk triage actions (archive/later/read/delete) over selected items from the inbox
- Keyboard-driven triage and navigation (j/k, Enter, a, l, r, s, o, x) via unobtrusive JavaScript
- Vendored, binary-embedded static assets (Pico CSS, HTMX) served from `/static` with an optional `--static-dir` override
- `pergamon-uniffi` crate: UniFFI facade exposing `pergamon-core` to Apple (Swift/SwiftUI) clients (spike #29)
- iOS sample app (`apps/ios`): SwiftUI app that lists and opens items served by the Rust core via UniFFI
- `scripts/build-ios.sh` and `scripts/smoke-macos.sh` for building the iOS XCFramework and running a host-side binding smoke test
- Spike findings doc on UniFFI ergonomics and binary size (`docs/spikes/uniffi-ios-findings.md`)
- Hardened UniFFI binding per ADR-019: flat `PergamonError` enum mapped to Swift `throws`, and a stateful `Library` object handle (`inbox`/`items`/`itemsWithStatus`/`item`/`search`) replacing the spike's free list/open functions (#113)
- `PergamonKit` Swift package (`apps/ios/PergamonKit`): idiomatic wrapper over the generated UniFFI bindings, with an XCTest suite runnable via `swift test`; the app consumes it with no hand-written FFI glue (#113)
- Production SwiftUI iPhone app (`apps/ios/Pergamon`) replacing the `PergamonSpike` sample: tab/navigation shell (Inbox, Saved, Search, Review), an `AppEnvironment` DI container owning the single `Library` handle, and a launch-time storage bootstrap (`StorageLocation`) that resolves the ADR-020 App Group container (`group.dev.pergamon`) and excludes the blob cache from backup, with a Simulator fallback so the app always launches (#114)
- iOS inbox and article reader views: inbox filtering by status, feed/source, and read/unread; per-item swipe actions (mark read/unread, save for later, archive); and an offline reader that renders the normalized extracted content served from the local core (#115)
- UniFFI surface additions for the iOS reader/triage flow: `ContentItem` now carries `content_text`, `read_at_millis`, and `source_name`; `Library` gains `mark_read`/`mark_unread`/`archive`/`save_for_later` mutations and a `sources()` helper (#115)
- macOS host slice added to `PergamonFFI.xcframework` so PergamonKit unit tests run natively on the host without a Simulator (#113)
- `pergamon-server` crate: Axum-based web server for pergamon (AGPL-3.0-only)
- REST API for content items: list, save URL, update status/tags, delete (`/api/items`)
- REST API for feeds: subscribe, list, delete, sync all, OPML import (`/api/feeds`)
- REST API for tags: list with item counts, add/remove tags on items (`/api/tags`)
- REST API for collections: create, list, view/add items (`/api/collections`)
- REST API for full-text search: ranked results with snippets and faceted filters — type, tag, status, source, date range (`/api/search`)
- REST API for saved searches backed by smart collections (`/api/saved-searches`)
- REST API for highlights: list with filters, per-item highlights, create, update note/color, delete (`/api/highlights`, `/api/items/:id/highlights`)
- REST API for notes: per-item list, create, update, delete (`/api/items/:id/notes`, `/api/notes/:id`)
- REST API for spaced-repetition review: due queue, submit FSRS rating, review statistics (`/api/review`)
- REST API for statistics: usage and review/retention reports (`/api/stats/usage`, `/api/stats/review`)
- Paginated list responses with `Link` headers and `X-Total-Count`
- Consistent JSON error responses with machine-readable error codes
- Health check endpoint at `/health` with database status
- Configurable host, port, database path, and static asset directory via CLI flags or environment variables
- Graceful shutdown on SIGINT/SIGTERM (Unix) or Ctrl+C (Windows)
- Gzip response compression and request tracing middleware
- URL save workflow with article extraction, metadata enrichment, and duplicate detection
- SSRF protection: HTTP/HTTPS-only URL validation, redirect limits, response size caps
- `pergamon-storage`: optional sort order (`ContentItemSort`: date, title, source) on filtered content-item listing
- Web Highlights view: source-grouped highlight browsing with tag/source/date/color filters, inline note editing, and JSON/Markdown export
- Web Notes view: note browsing with source context, note search, and inline create/edit/delete flows
- Web Review view: card-based review queue with reveal flow, Again/Hard/Good/Easy actions, and keyboard shortcuts (`Space`, `1`-`4`)
- Web Review stats dashboard with daily/weekly/monthly activity, retention indicators, and maturity distribution
- Web header navigation for Inbox, Highlights, Notes, Review, and Review stats pages
- Web Search view: full-text search with live results, faceted filters (type, status, tag, source, date range), highlighted result snippets, and save-as-smart-collection (#69)
- Recent searches on the Search view, remembered locally in the browser (#69)
- Web Bookmarks view: grid/list layouts, favicons and thumbnails, link-health badges, status filtering, pagination, and a quick-add form (#69)
- Web Tags view: weighted tag cloud plus a management table to rename, merge, and delete tags (#69)
- Web Collections view: browse regular and smart collections, create collections, edit smart-collection filters, and rename/delete collections (#69)
- Drag-and-drop reordering of items within a collection, with move up/down controls when JavaScript is unavailable (#69)
- Web header navigation links for Search, Bookmarks, Tags, and Collections pages (#69)
- Docker image for self-hosting the web server: multi-stage build, minimal `debian:bookworm-slim` runtime, runs as a non-root user, with `docker-compose.yml` and a `.dockerignore` (#71)
- `pergamon-server health-check` subcommand that probes the `/health` endpoint and exits non-zero when unhealthy; used as the container `HEALTHCHECK` so no `curl`/`wget` is needed in the image (#71)
- Self-hosting guide (`docs/docker.md`) covering quick start, configuration, data persistence, reverse-proxy (Caddy/nginx) setup, and backups (#71)

### Changed

- Review statistics API now includes monthly activity history (`monthly_history`) in `/api/review/stats`
- SQLite databases now open in WAL (Write-Ahead Logging) mode with `synchronous=NORMAL` and a 5s `busy_timeout`, allowing the web server and CLI to access the same database concurrently. WAL creates `*.db-wal` and `*.db-shm` sidecar files that are part of the live database — include them in raw file backups or use `export backup` while running (#83)

### Fixed

- Review submission now returns `404 Not Found` for unknown review cards instead of an internal error

## [0.6.1] - 2026-05-29

### Fixed

- Release matrix target.

## [0.6.0] - 2026-05-29

### Added

- Usage statistics and reading analytics: `stats usage` command with text and JSON output
- Articles read per day, week, and month with reading time estimates (238 WPM)
- Reading streaks: current and longest consecutive-day streaks
- Top content sources ranked by read count (feed-backed and URL-based)
- Tag distribution and monthly tag usage trends
- TUI usage statistics dashboard accessible from `stats usage --tui`
- `read_at` column tracking when items are marked as read (archived)
- `reading_time` module in `pergamon-core` for word count and reading time estimation

## [0.5.0] - 2026-05-29

### Added

- Content rules engine for automatic organization of incoming and existing items
- `rules add <name> --filter "..." --action "tag:foo"` command to create rules using the smart-filter DSL
- `rules list` command to display all defined rules with status, priority, and actions
- `rules remove <name-or-id>` command to delete rules
- `rules enable/disable <name-or-id>` commands to toggle rules without removing them
- `rules test --filter "..."` command to preview which items match a filter (read-only)
- `rules run [--dry-run]` command to apply all enabled rules against current inbox items
- Rule actions: `tag:<name>`, `status:<status>`, `collection:<name>`, `mute`
- Auto-tag, auto-archive, and source muting via rule definitions
- Rules automatically applied to newly ingested feed items and saved URLs
- Rule chaining: tag additions from earlier rules are visible to subsequent rules
- Protected status safety: auto-archive skips items marked as Later, Reference, or Reading
- Content rules included in backup/restore cycle

## [0.4.0] - 2026-05-29

### Added

- `pergamon-export` crate: new crate for structured export pipelines (Obsidian, future formats)
- `export obsidian` command: export highlights and bookmarks as Markdown notes into an Obsidian vault
  - `--vault <path>`: target vault directory
  - `--folder <name>`: subfolder within the vault (default: `Pergamon`)
  - `--dry-run`: preview files without writing
- Obsidian plugin (`apps/obsidian-plugin/`): TypeScript community plugin for browsing and inserting pergamon references
  - **Browse pergamon items**: fuzzy-search all exported highlights and bookmarks
  - **Insert pergamon reference**: insert wikilink, markdown link, or embed at cursor
  - **Reload manifest**: re-read the export manifest after a fresh export
  - **Show stats**: display item counts and last export time
  - Settings: configurable folder name, insert format, ribbon icon toggle
- Stable filename strategy: `{slug}--{uuid-prefix}.md` for deterministic, conflict-free file paths
- YAML frontmatter with proper escaping: handles quotes, backslashes, newlines, and YAML special characters
- Export manifest (`manifest.json`): JSON index of all exported items for plugin consumption
- Atomic manifest writes: temp file + rename to prevent partial reads
- Per-source highlight grouping: one Markdown note per source with all highlights and notes
- `pergamon stats review` top-level command: view retention and review statistics dashboard
- `review stats --format json` flag: machine-readable JSON output for review statistics
- `review stats --tui` / `stats review --tui` flag: launch a standalone TUI stats dashboard
- Review streak tracking: current and longest consecutive-day review streaks
- Source breakdown: review cards grouped by provenance (Kindle, Readwise, Feed, Manual)
- Daily review history: last 30 days of review activity with bar charts
- Weekly review trend: last 12 weeks of review activity with bar charts
- TUI stats dashboard: cards overview, retention/streaks panel, source breakdown, daily and weekly charts
- Review summary screen now shows `[s] Stats dashboard` option after completing a review session
- `export markdown` command: export content items as Markdown files with YAML frontmatter
  - Configurable filename templates with `{title}`, `{date}`, `{id}`, `{type}` placeholders
  - `--backlinks`: generate wikilink cross-references between related items
  - `--tag-format`: choose between YAML-only, hashtag, or both tag styles
  - `--type`: filter by content type
  - `--dry-run`: preview without writing
  - Automatic collision detection when template omits `{id}`
- `export json` command: export content items as versioned JSON with stable schema
  - Hierarchical structure: items with nested highlights, notes, bookmark and feed metadata
  - `--pretty`: human-readable formatting
  - `--include-content`: opt-in full content text
  - `--type`: filter by content type
  - Outputs to file or stdout
- Export format documentation (`docs/export-format.md`): schema reference, stability guarantees, examples
- Smart collections: rule-based dynamic collections using a filter DSL
  - `collection create <name> --smart "type:article tag:rust"`: create a smart collection
  - `collection edit-filter <id> <query>`: update a smart collection's filter
  - `collection show <id>`: displays dynamically matching items for smart collections
  - `collection list`: shows `[smart]` indicator and dynamic item counts
  - Filter DSL supports: `type:`, `tag:`, `status:`, `source:`, `since:`, `before:`, `text:` predicates
  - Predicates AND together; comma-separated values within a predicate OR together
  - Negation with `-status:discarded` syntax
  - Smart collections guard against manual `add`/`remove` operations
- Saved searches: create named smart collections from search queries
  - `search --save <name>`: save a search as a smart collection
  - `saved-search <name>`: re-run a saved search
  - `list-saved`: list all saved searches with their filters
- Smart filter DSL parser in `pergamon-core` (pure computation, zero I/O)
- `StorageError::Constraint` variant for smart collection guard errors
- V9 migration: `is_smart` and `filter_query` columns on collections table with trigger-based constraints
- TUI `SmartCollection` filter mode (preparatory for smart collection picker)

## [0.3.0] - 2026-05-28

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
- FSRS-5 spaced repetition engine in `pergamon-core` (pure computation, zero I/O): power forgetting curve, stability/difficulty updates, interval scheduling
- `ReviewCard` and `ReviewLog` domain types with full FSRS state tracking
- `review enable <id>` command: create a review card for any highlight
- `review disable <id>` command: remove a review card and its logs
- `review due` command: list cards due for review with configurable `--limit`
- `review stats` command: display aggregated review statistics (total cards, due count, retention rate, state breakdown)
- `review start` command: launch interactive TUI review session with Again/Hard/Good/Easy ratings
- TUI review mode: card display with source context, rating controls, progress bar, and session summary
- Review cards and review logs tables with FK cascades (V8 migration)
- Backup and restore now includes review cards and review logs
- `pergamon import kindle <file>` command: import highlights and notes from a Kindle My Clippings.txt file
- `pergamon import readwise <file>` command: import highlights from a Readwise CSV export with tags, source grouping, and provenance tracking
- Kindle parser: BOM-tolerant, handles highlights/notes/bookmarks, extracts title/author/location/date across Kindle device variants
- Readwise parser: flexible case-insensitive header matching, supports varying CSV column layouts
- `--dry-run` flag for Kindle and Readwise imports to preview changes without modifying the database
- `--enable-review` flag for Kindle and Readwise imports to auto-create FSRS review cards for imported highlights
- Idempotent re-import for Kindle and Readwise: duplicate detection via synthetic stable URLs (`kindle://` and `readwise://` schemes)
- Kindle notes imported as standalone notes attached to the source book
- Readwise source type mapping: books and articles to Article, podcasts to PodcastEpisode, PDFs to Pdf
- Readwise location field stored as highlight position for imported highlights
- Kindle note deduplication on re-import: skips notes with identical text already attached to the same source
- Transaction-wrapped imports for Kindle and Readwise: all inserts run in a single SQLite transaction for atomicity and performance
- `Database::in_transaction()`, `begin_transaction()`, `commit_transaction()`, `rollback_transaction()` public API in `pergamon-storage`

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
