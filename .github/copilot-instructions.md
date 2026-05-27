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

### What's NOT yet implemented

- Cargo workspace scaffold
- Any Rust code
- Any domain model, storage, or CLI functionality
- Obsidian plugin
- iOS/web clients
- Sync server

### Key files

**Documentation:**

- docs/roadmap.md — full product roadmap (20 sections)
- docs/adr/ — 10 Architecture Decision Records

## Reference Documents

The full product roadmap with all decisions, data model, and platform designs is in `docs/roadmap.md`. ADR index is in `docs/adr/README.md`.
