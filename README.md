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
pergamon review                                  # Spaced repetition session
pergamon import inoreader ~/subscriptions.opml   # Bring your feeds
pergamon import raindrop ~/export.csv            # Bring your bookmarks
pergamon export obsidian                         # Push highlights to Obsidian
```

> **Status**: Early development. Not yet usable.

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

## Features (Planned)

- 📡 RSS/Atom/JSON Feed subscription and reader mode
- 🔖 Bookmark management with nested collections, tags, and full-text search
- 📑 Read-later with full article extraction and offline reading
- 🧠 Spaced repetition resurfacing of highlights (FSRS algorithm)
- 📚 Kindle highlights import from My Clippings.txt
- 🔌 Obsidian plugin for syncing highlights and notes to your vault
- 📄 PDF import with text extraction
- 🔍 Full-text search across all content types (SQLite FTS5)
- 📥 Import from Inoreader, Raindrop.io, Readwise, Pocket, Instapaper, Newsboat
- 📤 Export to OPML, JSON, CSV, Markdown, Obsidian-compatible notes
- 🏗️ TUI with vim-style keybindings for reading, browsing, and reviewing

## Architecture

Cargo workspace following the [kafkade project DNA](https://github.com/kafkade):

```text
pergamon/
├── crates/
│   ├── pergamon-core/       # Domain models, state machine, SR engine (zero I/O)
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
| **Spaced repetition** | FSRS |
| **HTTP** | reqwest (rustls) |

## Documentation

- **[Product Roadmap](docs/roadmap.md)** — full phased roadmap with milestones
- **[Architecture Decision Records](docs/adr/)** — 10 ADRs covering core design choices

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
