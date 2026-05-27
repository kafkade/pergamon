## Description

<!-- What does this PR do? Provide a brief summary of the changes. -->

## Related Issues

<!-- Link related issues: "Closes #123" or "Relates to #456" -->

## Type of Change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Documentation update
- [ ] Refactoring (no functional changes)
- [ ] CI / infrastructure
- [ ] Other (describe below)

## Component

<!-- Which part of the monorepo does this touch? -->

- [ ] `crates/pergamon-core/` — Core library (domain model, content types, FSRS engine)
- [ ] `crates/pergamon-storage/` — SQLite + FTS5, migrations, blob store
- [ ] `crates/pergamon-feed/` — RSS/Atom/JSON Feed parsing, OPML
- [ ] `crates/pergamon-extract/` — Article extraction, HTML sanitization, PDF parsing
- [ ] `crates/pergamon-import/` — Importers (Inoreader, Raindrop, Readwise, Pocket, Kindle)
- [ ] `crates/pergamon-export/` — Exporters (OPML, JSON, CSV, Markdown, Obsidian)
- [ ] `crates/pergamon-cli/` — CLI tool (clap + ratatui TUI)
- [ ] `apps/obsidian-plugin/` — Obsidian community plugin
- [ ] `docs/` — Documentation

## Domain

<!-- Which content domain(s) does this touch? -->

- [ ] Feed management (RSS/Atom/OPML)
- [ ] Article extraction / reader mode
- [ ] Bookmark management / collections
- [ ] Highlights / annotations
- [ ] Spaced repetition / review
- [ ] Full-text search
- [ ] Import / export
- [ ] Obsidian integration
- [ ] Cross-domain

## Checklist

- [ ] I have read [CONTRIBUTING.md](CONTRIBUTING.md)
- [ ] Tests pass (`cargo test --workspace`)
- [ ] Clippy passes (`cargo clippy --workspace -- -D warnings`)
- [ ] Formatting passes (`cargo fmt --check`)
- [ ] I have updated documentation (if applicable)
