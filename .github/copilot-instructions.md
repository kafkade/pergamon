# Copilot Instructions for pergamon

## Project Overview

pergamon is a unified personal information system that replaces four traditionally separate tools — RSS reader, read-later service, bookmark manager, and knowledge retention engine — with a single local-first, CLI/TUI-first system. It targets power users who have outgrown Inoreader, Readwise, Readwise Reader, and Raindrop.io, and want to own their data.

**Stack**: Rust core library + CLI (clap/ratatui TUI) + future iOS (SwiftUI via UniFFI) + future web (WASM) + optional sync server (Axum, AGPL-3.0).

## Architecture

### Monorepo Layout

- `crates/pergamon-core/` — Shared Rust library: domain model (documents, feeds, collections, tags, highlights, review cards), content type taxonomy, state machine, spaced repetition engine (FSRS). **No I/O, no networking** — pure computation only.
- `crates/pergamon-storage/` — SQLite + FTS5 storage implementation, migrations, content-addressed blob store. Implements storage traits from pergamon-core.
- `crates/pergamon-feed/` — RSS/Atom/JSON Feed parsing (via feed-rs), OPML import/export, feed discovery.
- `crates/pergamon-extract/` — Article extraction (readability algorithm), HTML sanitization (ammonia), PDF text extraction.
- `crates/pergamon-import/` — Importers: Inoreader (OPML), Raindrop.io (CSV/JSON), Readwise (CSV), Pocket (HTML), Instapaper, Newsboat (URLs), Kindle (My Clippings.txt).
- `crates/pergamon-export/` — Exporters: OPML, JSON, CSV, Markdown, Obsidian-compatible notes.
- `crates/pergamon-cli/` — CLI binary (clap commands + ratatui TUI). Only place in Rust that does HTTP (via reqwest).
- `apps/obsidian-plugin/` — Obsidian community plugin (TypeScript). One-way sync: pergamon → vault.

### Key Design Constraints

1. **pergamon-core must have zero I/O dependencies.** No `reqwest`, no file system access, no platform APIs. All I/O happens in platform-specific code. This keeps the core testable (pure functions) and compilable to WASM.

2. **Unified content model.** All content types (feed items, articles, bookmarks, highlights, PDFs, newsletters) share a single `document` entity with a `content_type` discriminator. Content type is a filter, not a silo.

3. **Local-first.** The local SQLite database is the canonical store. Sync is optional and always client-initiated. No accounts, no cloud dependency.

4. **Reader mode stores both forms.** Raw HTML snapshot plus normalized extracted article text. This preserves reprocessing options and offline reading quality.

5. **Spaced repetition is core, not an add-on.** Highlights and notes can become review cards using the FSRS algorithm. The data model supports this from day one.

### Content Model

The canonical unit of saved content is a `document`:

```text
document (stable identity)
├── source_subscription (optional feed/newsletter link)
├── raw_blob (HTML snapshot, PDF, email)
├── extracted_content (normalized text, structure)
├── annotations[] (highlights, notes)
└── review_cards[] (FSRS-scheduled cards derived from annotations)
```

Collections and tags provide organization. A triage workflow moves documents through states: `inbox → later/reference → reading → archived/discarded`.

## Conventions

### Error Handling

- Use `thiserror` for library errors in core crates
- Use `anyhow` only in binary crates (`pergamon-cli`)
- Wrap errors with context (`anyhow::Context`)

### Licensing

Everything is Apache-2.0 except the future `pergamon-server` which will be AGPL-3.0. Don't move server code into pergamon-core or vice versa without considering license implications.

## Build & Test

```sh
# Build all crates
cargo build --workspace

# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p pergamon-core

# Run a single test by name
cargo test -p pergamon-core test_name

# Clippy lints
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --check
```

## ADRs

Architecture Decision Records live in `docs/adr/`. Read them before making changes to:

- Zero-I/O core (ADR-001)
- Content type taxonomy and unified data model (ADR-002)
- Feed parsing strategy — feed-rs (ADR-003)
- Content extraction — readability + ammonia (ADR-004)
- Spaced repetition algorithm — FSRS (ADR-005)
- Database schema and FTS5 strategy (ADR-006)
- Platform boundaries (ADR-007)
- Licensing — Apache-2.0 + AGPL-3.0 (ADR-008)
- CLI/TUI design and command structure (ADR-009)
- Bookmark vs article — unified content model (ADR-010)

## Git Policy

- **Never modify git history.** Do not run any command that creates, modifies,
  or deletes commits, refs, or tags. This includes but is not limited to:
  `git commit`, `git push`, `git rebase`, `git merge`, `git cherry-pick`,
  `git revert`, `git reset`, `git tag`, `git am`, `git stash drop`.
- **Read-only git is fine.** Commands that only inspect state are permitted:
  `git status`, `git diff`, `git log`, `git show`, `git branch --list`,
  `git stash list`, `git rev-parse`, etc.
- **Staging is fine.** `git add` and `git stash push` are permitted for
  preparing diffs or preserving work, since they don't alter commit history.
- Always present proposed changes and let the user decide when to commit.
- This applies to **all** agents, sub-agents, and automated workflows —
  no exceptions, including CI-related or "cleanup" commits.

## CI / Infrastructure Dependency

**Branch protection for this repo is managed via Terraform in `kafkade/github-infra` (`repos/pergamon/`).** The `required_status_checks` list must match the job names in `.github/workflows/ci.yml`. If you rename, add, or remove CI jobs that are used as merge gates, the corresponding IaC config must be updated or PRs will be permanently blocked. Always flag this when proposing workflow changes.

## PR Title Format

Use conventional commits: `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`. For multi-component changes, include the primary component: `feat(feed): add OPML import parser`.

## Implementation Status

### What's implemented

- Repository scaffolding: GitHub templates, CI workflow, copilot instructions, contribution guide, and licensing
- Architecture Decision Records (`docs/adr/ADR-001` through `ADR-010`)
- Product roadmap (`docs/roadmap.md`)
- Cargo workspace with six crates: `pergamon-core`, `pergamon-storage`, `pergamon-feed`, `pergamon-extract`, `pergamon-import`, `pergamon-cli`
- Unified content model: domain types for content items, feeds, feed folders, tags, collections, highlights, and bookmarks (`pergamon-core`)
- SQLite schema with FTS5 full-text search (8 migrations), extension tables, `updated_at` triggers (`pergamon-storage`)
- Custom embedded migration runner for schema versioning
- CRUD operations for all content entities with filtered listing and full-text search
- Feed subscription and sync engine: `feed add/list/refresh/remove`, `sync` commands
- RSS/Atom/JSON Feed parsing via feed-rs, conditional GET (ETag/Last-Modified), feed health tracking
- OPML import/export with folder hierarchy, dry-run mode, and idempotent re-import
- Feed folder management: `feed move`, `feed list --tree`
- Article extraction pipeline: readability + ammonia HTML sanitization (`pergamon-extract`)
- Metadata extraction from Open Graph, Twitter Card, and standard meta tags
- PDF text-layer extraction via lopdf
- URL canonicalization for deduplication (`pergamon-extract`)
- `save <url>` command with `--tag`, `--bookmark`, pipe support, and duplicate detection
- `read` command: TUI inbox and article reader (ratatui) with vim-style keybindings
- TUI triage workflow: filter by status/feed/folder, quick filters (0-5), bulk mark-as-read, browser open
- `search <query>` command: FTS5 search with faceted filters (`--type/--tag/--status/--source/--since/--before`), JSON output
- TUI search: `/` key, search input bar, `FilterMode::Search`
- Full backup export/restore: `export backup` (ZIP with JSON) and `import backup` with transactional restore
- Configuration file support: `config` command, TOML config at platform config dir
- Shell completions: `completions <shell>` for bash/zsh/fish/powershell
- Test fixture corpus: 25 feed fixtures, 29 extraction HTML fixtures, corpus integration tests
- Collections, tags, and bulk organization: `collection` and `tag` command groups, nested collections, bulk tag/move/archive/delete (#13)
- Raindrop.io CSV import and Pocket HTML import with dry-run and idempotent re-import (#14)
- Metadata enrichment: favicon, JSON-LD author, `og:site_name`, Twitter Card fallback (#15)
- Duplicate detection (`doctor dupes`) and merge (`doctor merge`) with canonical URL matching (#16)
- Link health checking (`doctor links`) with `--stale` flag for incremental re-checking (#17)
- Highlight and note model: `highlight add/list/show/export`, `note add/list/edit/delete`, TUI highlight capture (#18)
- Notes table with cascade deletion (V7 migration), highlights searchable via FTS
- FSRS-5 spaced repetition engine in `pergamon-core` (pure computation, zero I/O) (#19)
- Review cards and review logs: `review enable/disable/due/stats/start` commands, TUI review mode (#19)
- Review cards and review logs tables with FK cascades (V8 migration)
- Kindle My Clippings.txt import: `import kindle` with BOM handling, multi-format date parsing, highlight/note dedup (#20)
- Readwise CSV export import: `import readwise` with flexible headers, source type mapping, tag preservation (#21)
- `--dry-run` and `--enable-review` flags for Kindle and Readwise imports
- Idempotent re-import via synthetic stable URLs (`kindle://` and `readwise://` schemes)
- Transaction-wrapped bulk imports for atomicity and performance
- Backup and restore includes notes, review cards, and review logs

### What's NOT yet implemented

- Obsidian plugin (#23)
- Stable export contracts (#24)
- Smart collections, content rules, analytics (#25-#27)
- WASM/UniFFI spikes (#28-#29)
- iOS/web clients (#33-#34)
- Sync server (#35)

### Key files

**Crates:**

- `crates/pergamon-core/` — Domain model, content types, status enum, FSRS-5 spaced repetition engine (zero I/O)
- `crates/pergamon-storage/` — SQLite + FTS5, 8 migrations, CRUD operations, transaction API
- `crates/pergamon-feed/` — RSS/Atom/JSON Feed parsing, OPML import/export
- `crates/pergamon-extract/` — Article extraction, HTML sanitization, URL canonicalization
- `crates/pergamon-import/` — Importers: OPML, Raindrop.io (CSV), Pocket (HTML), Kindle (My Clippings.txt), Readwise (CSV)
- `crates/pergamon-cli/` — CLI binary (clap), TUI (ratatui), HTTP (reqwest)

**Documentation:**

- docs/roadmap.md — full product roadmap (20 sections)
- docs/adr/ — 10 Architecture Decision Records

## Reference Documents

The full product roadmap with all decisions, data model, and platform designs is in `docs/roadmap.md`. ADR index is in `docs/adr/README.md`.
