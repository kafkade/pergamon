# 📡 Pergamon

**A unified personal information system.**

Subscribe to feeds. Save articles. Bookmark the web. Retain what matters. Own every byte.

Pergamon replaces four paid services with one local-first CLI tool:
[Inoreader](https://www.inoreader.com/) (RSS),
[Readwise](https://readwise.io/) (highlights & spaced repetition),
[Readwise Reader](https://readwise.io/read) (read-later),
and [Raindrop.io](https://raindrop.io/) (bookmarks) — with an
[Obsidian](https://obsidian.md/) plugin to bridge it all into your knowledge base.

```sh
pergamon feed add https://example.com/feed.xml  # Subscribe to a feed
pergamon feed refresh                            # Fetch new articles
pergamon read                                    # Open the TUI reader
pergamon save https://example.com/article        # Save for later
pergamon search "distributed systems"            # Full-text search everything
pergamon highlight add <id> --quote "key idea"   # Capture a highlight
pergamon review start                            # Spaced repetition session
pergamon import opml ~/subscriptions.opml        # Bring your feeds
pergamon import raindrop ~/export.csv            # Bring your bookmarks
pergamon import kindle ~/My\ Clippings.txt       # Bring your Kindle highlights
pergamon import readwise ~/readwise-export.csv   # Bring your Readwise highlights
pergamon export backup -o library.zip            # Full backup
```

> **Status**: Active development — Phase 3 complete. Core reading, bookmarking, organization, highlights, spaced repetition, Kindle/Readwise import, Obsidian integration, and stable export contracts are implemented.

---

## Why "Pergamon"?

The [Library of Pergamon](https://en.wikipedia.org/wiki/Library_of_Pergamon) was the
second-greatest library of the ancient world — rivaling Alexandria itself. Built in the
3rd century BCE in the city of Pergamon (modern-day Bergama, Turkey), it housed over
200,000 scrolls and became a center for scholarship, curation, and the preservation of
knowledge.

When Egypt banned the export of papyrus to undermine Pergamon's growing collection, the
scholars of Pergamon didn't give up — they invented **parchment** (_pergamena_ in Latin,
literally "of Pergamon"), a new writing medium made from animal skin. The very word
"parchment" descends from the name of this library.

The name captures what this project is about:

- **Curation over consumption** — Pergamon's librarians didn't just collect scrolls; they
  organized, annotated, and preserved them for future scholars. This app doesn't just
  save links — it extracts, archives, tags, and resurfaces knowledge.
- **Resilience and ownership** — When the supply of papyrus was cut off, Pergamon
  invented its own medium rather than depend on a foreign power. When cloud services
  shut down (see: Omnivore, Google Reader), your data should survive. Everything lives
  locally, in formats you control.
- **A personal library** — Pergamon was a working library, not a showroom. No social
  features, no followers, no feeds of feeds. Just you and your collection.
- **Short enough for a CLI** — eight characters, distinctive, no namespace conflicts.
  `pergamon feed add` rolls off the keyboard.

---

## Principles

- **Your data, your machine.** Everything lives in a local SQLite database. No accounts,
  no servers, no cloud required. Export everything at any time.
- **No social features.** No sharing, following, or collaborative collections. This is a
  personal tool for a personal library.
- **One library, not five apps.** Feeds, articles, bookmarks, highlights, and PDFs flow
  into a single searchable, taggable collection. Content type is a filter, not a silo.
- **Import everything.** Bring your Inoreader feeds, Raindrop bookmarks, Readwise
  highlights, Pocket saves, and Kindle clippings. Years of curation should transfer in
  minutes.
- **CLI/TUI-first.** A fast, keyboard-driven terminal interface powered by ratatui.
  Web and iOS come later — built on the same Rust core.
- **Retain, don't just save.** Built-in spaced repetition resurfaces your highlights so
  knowledge sticks — not just accumulates.
- **Open source.** Apache-2.0 licensed. Contributions welcome.

## Features

### Implemented

- 📡 RSS/Atom/JSON Feed subscription with conditional GET, feed health tracking, and folder management
- 📑 Read-later with full article extraction, offline reading, and TUI reader with vim-style keybindings
- 🔖 Bookmark management with nested collections, tags, bulk operations, and full-text search
- 🔍 Full-text search across all content types (SQLite FTS5) with faceted filters
- ✏️ Highlights and notes: create, list, search, export, and TUI capture
- 🧠 Spaced repetition with FSRS-5 algorithm: review queue, interactive TUI sessions, retention stats
- 📚 Kindle My Clippings.txt import with highlight and note extraction
- 📥 Readwise CSV import with tags, source grouping, and provenance tracking
- 📥 Import from OPML, Raindrop.io (CSV), and Pocket (HTML) with dry-run and idempotent re-import
- 📤 Export: OPML feeds, full backup (ZIP with JSON), highlight export (Markdown/JSON)
- 📝 Stable export contracts: general-purpose Markdown (frontmatter, backlinks, slug templates) and versioned JSON
- 🔌 Obsidian plugin for syncing highlights and notes to your vault
- 🔗 URL canonicalization, duplicate detection, and link health checking
- 🏗️ TUI with vim-style keybindings for reading, triage, highlighting, and reviewing
- ⚙️ Configuration file (TOML), shell completions (bash/zsh/fish/PowerShell)

### Planned

- 📧 Newsletter ingestion (IMAP, `.eml` import)
- 🤖 Smart collections, content rules, and analytics
- 🌐 Web interface (Axum + WASM)
- 📱 iOS client (SwiftUI via UniFFI)
- 🔄 Cross-device sync server

## Architecture

Cargo workspace following the [kafkade project DNA](https://github.com/kafkade):

```text
pergamon/
├── crates/
│   ├── pergamon-core/       # Domain models, state machine, FSRS-5 engine (zero I/O)
│   ├── pergamon-storage/    # SQLite + FTS5, migrations, content archival
│   ├── pergamon-feed/       # RSS/Atom/JSON Feed parsing, OPML, discovery
│   ├── pergamon-extract/    # Article extraction, PDF parsing, HTML sanitization
│   ├── pergamon-import/     # Importers (Inoreader, Raindrop, Readwise, Pocket, Kindle)
│   ├── pergamon-export/     # Exporters (OPML, JSON, CSV, Markdown, Obsidian)
│   └── pergamon-cli/        # CLI + TUI binary (clap + ratatui)
├── apps/
│   └── obsidian-plugin/     # Obsidian community plugin (TypeScript)
└── docs/
    ├── roadmap.md           # Full product roadmap
    └── adr/                 # Architecture Decision Records
```

## Tech Stack

| Component | Choice |
|-----------|--------|
| **Language** | Rust |
| **Database** | SQLite with FTS5 |
| **CLI** | clap v4 |
| **TUI** | ratatui + crossterm |
| **Feed parsing** | feed-rs |
| **Article extraction** | readability + ammonia |
| **Spaced repetition** | FSRS-5 (pure Rust implementation) |
| **HTTP** | reqwest (rustls) |

## Self-hosting with Docker

Run the pergamon web server in a container with persistent storage:

```sh
docker compose up -d
# server on http://localhost:3000
```

See **[docs/docker.md](docs/docker.md)** for configuration, data persistence,
reverse-proxy (TLS) setup, and backups.

## Documentation

- **[Product Roadmap](docs/roadmap.md)** — full phased roadmap with milestones
- **[Architecture Decision Records](docs/adr/)** — ADRs covering core design choices
- **[Docker / self-hosting guide](docs/docker.md)** — run the web server with Docker

## Related Projects

Pergamon is part of the [kafkade](https://github.com/kafkade) ecosystem of local-first
personal tools:

| Project | Purpose |
|---------|---------|
| [toku](https://github.com/kafkade/toku) | Personal book manager |
| [kora](https://github.com/kafkade/kora) | Terminal audio player |
| [tock](https://github.com/kafkade/tock) | Productivity engine (tasks, habits, time) |
| [ldgr](https://github.com/kafkade/ldgr) | Zero-knowledge personal finance |

## License

[Apache-2.0](LICENSE)
