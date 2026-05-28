# 📡 pergamon — Product Roadmap

> **A unified personal information system — RSS reader, read-later, bookmark manager, and knowledge retention engine.**
> CLI/TUI-first. Local-first. Your data, your rules. No social features. No cloud dependency.

**Repository**: `kafkade/pergamon`
**Core Language**: Rust
**Primary Interface**: CLI + ratatui TUI
**Date**: May 2026
**Author**: kafkade

---

## Section 0: Clarifying Questions

The roadmap below assumes a **local-first, CLI/TUI-first knowledge ingestion system** whose job is to ingest, normalize, search, annotate, review, and resurface personal reading inputs. The table answers the 25 core product-definition questions with a single recommended path for each.

### Assumptions Table

| # | Clarifying question | Recommended path | Why this path | Complexity | Status | Risk if wrong |
|---|---|---|---|---|---|---|
| 1 | What is pergamon’s core job? | **Be the canonical local library for personal reading inputs and derived knowledge.** | It keeps the product coherent: ingestion, search, annotation, review, and resurfacing all belong to one system. | 🟢 | [Validated] | If pergamon becomes “everything PKM,” scope explodes and quality drops. |
| 2 | Who is the first release for? | **Power users leaving Inoreader + Readwise + Reader + Raindrop.** | They feel the pain sharply enough to tolerate a CLI/TUI-first v1 and provide high-quality feedback. | 🟢 | [Validation Required] | If the real audience is casual users, CLI-first sequencing is wrong. |
| 3 | What is the development model? | **Solo-developer, milestone-driven, 3–5 meaningful deliverables per phase.** | This matches kafkade project DNA and prevents architectural overreach. | 🟢 | [Validated] | Overscoped phases will stall the project before cross-platform reuse pays off. |
| 4 | What is the license split? | **Apache-2.0 for all client/core crates; AGPL-3.0 for `pergamon-server`.** | This mirrors the ldgr/tock pattern and keeps server-side improvements open. | 🟢 | [Validated] | License churn later is painful and can fracture contributions. |
| 5 | What platforms ship first? | **Desktop CLI + ratatui TUI first; iOS second; web third; no native desktop GUI pre-1.0.** | The desktop terminal is the fastest way to validate the data model and workflows. | 🟡 | [Validated] | If mobile capture is more critical than expected, adoption may lag until sync/iOS arrive. |
| 6 | Which content types are in scope? | **RSS/Atom feeds, web articles, email newsletters, PDFs, Kindle highlights.** | These are the user-confirmed replacement targets and form a coherent ingestion graph. | 🟢 | [Validated] | Adding more types early dilutes the ingestion architecture. |
| 7 | Which content types are explicitly out of scope? | **No podcast playback, no social features, no collaborative libraries.** | Podcast playback belongs in kora; social features violate the product’s personal/local-first identity. | 🟢 | [Validated] | If ignored, the roadmap sprawls into unrelated product categories. |
| 8 | What is the canonical unit of saved content? | **A `document` with a stable identity, optional source subscription, optional raw blobs, and derived annotations/cards.** | One model must cover feed items, articles, newsletters, PDFs, and Kindle-derived book excerpts. | 🟡 | [Validation Required] | If identity is per-source instead of per-document, deduplication becomes messy. |
| 9 | How should feed support work? | **Support RSS/Atom ingestion plus OPML import/export from day one.** | RSS/Atom covers the real standard surface; OPML is required for migration from Inoreader. | 🟢 | [Validation Required] | Without OPML, “replace Inoreader” is not credible. |
| 10 | How should newsletters be ingested? | **Read-only IMAP sync plus `.eml` import; no hosted inbound email service.** | It preserves local-first values and avoids running an always-on mail gateway. | 🟡 | [Validation Required] | Some users may want “send to pergamon” convenience; that can come later via plugin/forwarder. |
| 11 | What is the PDF policy? | **Support local PDFs and web-captured PDFs with text-layer extraction; no OCR in v1.** | Text PDFs cover the majority of useful research/document workflows without pulling in OCR complexity. | 🟡 | [Validated] | Researchers with scanned PDFs will hit the ceiling quickly. |
| 12 | How should Kindle highlights enter the system? | **Import from `My Clippings.txt` and exported Kindle notebook formats; no Kindle cloud scraping or DRM-adjacent behavior.** | It is legally cleaner, deterministic, and aligned with open-source/local-first ethics. | 🟡 | [Validation Required] | If Kindle exports are too lossy, some users will keep Readwise longer. |
| 13 | How should reader mode work? | **Store raw HTML snapshot plus normalized extracted article text and structure.** | Keeping both original and normalized forms preserves reprocessing options and offline reading quality. | 🔴 | [Validation Required] | If only extracted text is stored, fidelity suffers; if only raw HTML is stored, search and reading are poor. |
| 14 | What is the bookmarking model? | **Raindrop-style collections + tags, with bookmarks as first-class saved items that may optionally have extracted content.** | This cleanly separates “I saved a link” from “I extracted an article,” while still unifying both in one library. | 🟡 | [Validated] | If bookmarks are just another article subtype, quick capture/reference workflows become awkward. |
| 15 | What is the read-later workflow? | **A triage queue: `inbox -> later/reference -> reading -> archived/discarded`.** | Users replacing Reader/Raindrop need more than storage; they need flow control. | 🟡 | [Validation Required] | If state is too simple, the queue becomes a pile instead of a workflow. |
| 16 | Is spaced repetition truly core? | **Yes: highlights and notes can become review cards, and resurfacing is a first-class workflow.** | This is the clearest Readwise replacement wedge and a durable differentiator. | 🔴 | [Validated] | If SRS is bolted on later, data model and annotation history will need painful redesign. |
| 17 | Should AI be required for summarization/cards? | **No mandatory AI dependency; deterministic extraction and manual/templated card generation first.** | Core knowledge retention must work offline and remain reproducible. | 🟡 | [Validation Required] | If users expect AI summaries immediately, some may see v1 as less magical than incumbents. |
| 18 | What is the Obsidian direction? | **pergamon is the source of truth; the Obsidian plugin mirrors/export-syncs selected content into a vault. One-way first.** | Bidirectional editing is seductive but creates hard conflict semantics too early. | 🟡 | [Validated] | If heavy Obsidian users expect true two-way sync, they may see the first integration as limited. |
| 19 | How should search work? | **SQLite + FTS5 for canonical local search, with field filters and saved queries layered above it.** | It fits the stack, stays portable, and keeps search available offline on every platform. | 🟢 | [Validated] | If search semantics differ by platform, user trust collapses. |
| 20 | What is the encryption stance? | **End-to-end encrypt sync payloads and remote blobs; keep local DB plaintext by default, relying on OS disk encryption initially.** | pergamon needs local search and multi-platform simplicity more than full local vault complexity on day one. | 🔴 | [Validated] | Some privacy-maximalist users may insist on local at-rest encryption from the start. |
| 21 | What is the sync architecture? | **SQLite remains canonical; local mutations emit batched sync events and snapshots in the ldgr/tock style.** | This preserves local-first behavior while avoiding full event-sourcing complexity. | 🔴 | [Validated] | If the event model is underspecified, cross-device conflict handling will become brittle. |
| 22 | How should conflicts be resolved? | **Hybrid policy: append-only events merge automatically; concurrent edits to the same note/card body create a conflict inbox item.** | Knowledge data is less strict than finance, but silent note loss is unacceptable. | 🔴 | [Validation Required] | Too much automatic merge loses trust; too much manual review kills usability. |
| 23 | Where should content live? | **Metadata, normalized text, annotations, cards, and FTS indexes in SQLite; raw HTML/PDF/email blobs in a content-addressed blob store.** | This gives portable search plus efficient storage of large immutable binaries. | 🟡 | [Validated] | If binaries are shoved into SQLite, backups and sync become unnecessarily heavy. |
| 24 | What storage budget should the system target? | **Desktop target: 10k documents + 2k PDFs + 100k annotations within a managed 5 GB default blob budget; mobile defaults to a much smaller cache.** | It is ambitious enough for power users without forcing object storage or complex tiering early. | 🟡 | [Validation Required] | If real-world libraries exceed this quickly, cache/retention controls must arrive sooner. |
| 25 | What architectural conventions must pergamon inherit from sibling projects? | **Zero-I/O core, clap v4, ratatui TUI, SQLite+FTS5, UniFFI for iOS, WASM for web, Axum sync server, strict local-first boundaries.** | Reusing proven project DNA lowers maintenance cost and keeps the portfolio internally coherent. | 🟢 | [Validated] | Deviating without a compelling reason increases long-term cognitive load and tooling drift. |

---

## Section 1: User Personas

The personas below are deliberately opinionated. They are not marketing archetypes; they are **roadmap anchors**. If a proposed feature does not materially improve life for at least one of these users, it should not outrank ingestion fidelity, local-first search, or review workflows.

### Phase legend used below

| Phase | Working name | What it unlocks |
|---|---|---|
| **Phase 1** | Foundations, CLI/TUI, core ingestion | OPML/feed import, manual save, local library, search, collections/tags |
| **Phase 2** | Reader-depth and capture fidelity | Better article extraction, newsletter ingestion, PDF ingestion, offline reading |
| **Phase 3** | Highlights, notes, spaced repetition | Annotation pipeline, review queues, resurfacing, knowledge retention |
| **Phase 4** | Sync + iOS | Cross-device capture, mobile reading/review, encrypted sync |
| **Phase 5** | Web + Obsidian polish | Browser access, vault integration, broader portability |

### 1. Nadiya — The Feed Maximalist  

**Primary wedge persona**

- **Archetype**: Former Inoreader power user who follows dozens to hundreds of feeds across tech, research, essays, and niche blogs.
- **Content volume**: 150–400 subscriptions, 200–600 unread items at any given time, 20–50 articles worth archiving per week.
- **Technical comfort**: High. Comfortable with terminal tools, exports/imports, and self-hosted utilities. Not afraid of config files.
- **Platforms**: macOS or Linux terminal first; iPhone later for triage on the move.
- **Current stack**: Inoreader for feeds, Reader for long reads, Raindrop for links worth keeping.
- **Pain points**:
  - Feed readers are good at *surfacing* content but bad at *retaining* it.
  - Saved articles get fragmented across multiple services.
  - Collections/tags in bookmark tools are divorced from feed context.
  - Export is possible but not pleasant; ownership is partial, not total.
- **Switching trigger**: `pergamon import opml subscriptions.opml` followed by a usable TUI inbox, reliable unread/archive semantics, and first-class saved-item organization.
- **What “done” looks like for Nadiya**:
  - Feeds sync locally and efficiently.
  - She can save an item from a feed into a collection without losing source metadata.
  - Search spans both feed items and archived long-form content.
  - She never needs to ask “did I save this in Inoreader, Reader, or Raindrop?”
- **Roadmap phase fit**: **Phase 1** is decisive; **Phase 2** makes the switch durable; **Phase 4** broadens daily use.

### 2. Marco — The Read-Later Triage Operator  

**Reader replacement persona**

- **Archetype**: Knowledge worker who captures aggressively and reads selectively. Saves far more than he finishes.
- **Content volume**: 100–300 saved links/articles per month, 500–1,500 item backlog, frequent pruning and resurfacing.
- **Technical comfort**: Medium-high. Comfortable with desktop apps and automation; may not live in the terminal but respects tools that are scriptable.
- **Platforms**: Desktop first, iPhone second, web third.
- **Current stack**: Readwise Reader or Instapaper/Pocket-like behavior layered on top of a bookmark manager.
- **Pain points**:
  - Read-later tools become graveyards without strong triage states.
  - Bookmark apps are good for reference, but weak for actual reading flow.
  - Archiving and resurfacing are separate systems.
  - “Save” is easy; “find again and act on it” is not.
- **Switching trigger**: pergamon offers a clear queue model—**inbox, later, reference, reading, archived, discarded**—plus strong keyboard-first triage in the TUI.
- **What “done” looks like for Marco**:
  - Saved links and extracted articles live in the same library.
  - “Reference” and “read later” are different intents, not the same pile.
  - Resurfacing is driven by actual highlights/notes, not just random recirculation.
  - Offline snapshots prevent dead-link anxiety.
- **Roadmap phase fit**: **Phase 2** is his real unlock; **Phase 3** converts convenience into retention.

### 3. Priya — The Obsidian Knowledge Gardener  

**Obsidian integration persona**

- **Archetype**: Research-driven knowledge worker who reads to synthesize, not just consume.
- **Content volume**: 50–150 important documents per month, 500–5,000 evergreen notes in Obsidian, heavy highlighting/note extraction.
- **Technical comfort**: High. Comfortable with Markdown, frontmatter, plugins, and opinionated workflows.
- **Platforms**: macOS/iPad/iPhone; occasionally web.
- **Current stack**: Obsidian + Readwise + Reader + ad hoc capture tools.
- **Pain points**:
  - Highlights live in one system while durable notes live in another.
  - Readwise export often feels like a firehose, not a curated knowledge bridge.
  - Bidirectional note systems become fragile when schemas drift.
  - She wants stable links and metadata, not magic sync that occasionally corrupts.
- **Switching trigger**: A high-quality Obsidian plugin that mirrors selected pergamon documents, annotations, and review metadata into the vault with predictable filenames, frontmatter, backlinks, and deep links back to pergamon.
- **What “done” looks like for Priya**:
  - pergamon remains the ingestion and review engine.
  - Obsidian remains the synthesis and authorship environment.
  - She can trust stable IDs, stable paths, and deterministic re-exports.
  - Notes added in Obsidian do not silently mutate pergamon’s canonical content.
- **Roadmap phase fit**: **Phase 3** starts to matter; **Phase 5** is her full unlock.

### 4. Elias — The Newsletter Archivist  

**Email ingestion persona**

- **Archetype**: Follows many high-signal newsletters and wants them treated as first-class reading material, not as disposable inbox clutter.
- **Content volume**: 20–80 newsletters per week, 500+ archived issues over time, moderate tagging and selective extraction.
- **Technical comfort**: Medium. Comfortable enough for IMAP credentials and setup if the payoff is obvious.
- **Platforms**: Desktop first; mobile for casual reading later.
- **Current stack**: Email inbox folders, maybe plus Reader or a bookmark app for the “important” ones.
- **Pain points**:
  - Newsletters compete with personal/work email and disappear into inbox entropy.
  - Good issues are hard to resurface later by topic or author.
  - Email clients are not knowledge systems.
  - Saved copies often lose structure or become ugly HTML fragments.
- **Switching trigger**: Read-only IMAP import that pulls newsletter issues into pergamon as searchable documents with sender/source metadata, tags, collections, and offline readability.
- **What “done” looks like for Elias**:
  - Newsletters are separated from transactional mail.
  - Search works across senders, topics, highlights, and extracted text.
  - Archived issues feel like part of the library, not part of the inbox.
- **Roadmap phase fit**: **Phase 2** is essential; **Phase 4** makes it habitual.

### 5. Dr. Mina Hassan — The PDF-Heavy Researcher  

**PDF and annotation persona**

- **Archetype**: Researcher, analyst, consultant, or policy reader who consumes long PDFs, reports, papers, and whitepapers.
- **Content volume**: 50–200 PDFs per quarter, frequent highlighting, many partially read documents, moderate metadata curation.
- **Technical comfort**: Medium-high. Comfortable with files, citations, exports, and structured tools.
- **Platforms**: macOS/Linux desktop first; iPad or iPhone later for reading/review.
- **Current stack**: Finder/Downloads + PDF app + bookmark tool + maybe Zotero for some subsets.
- **Pain points**:
  - PDF reading and web/article reading are split into different worlds.
  - Search across local PDFs and saved articles is fragmented.
  - Highlights are hard to consolidate unless trapped in proprietary viewers.
  - She wants one review pipeline, not one per document type.
- **Switching trigger**: pergamon can ingest PDFs, extract text where available, preserve originals, and put annotations into the same search/review system as articles and newsletters.
- **What “done” looks like for Mina**:
  - PDFs feel like first-class library items, not attached files.
  - Text-layer extraction enables search, snippets, and card generation.
  - A PDF highlight can sit beside a web article highlight in the same review queue.
- **Roadmap phase fit**: **Phase 2** unlocks storage and reading value; **Phase 3** unlocks retention value.

### 6. Jonah — The Kindle Highlight Migrant  

**Readwise replacement persona**

- **Archetype**: Book reader who values highlight retention more than glossy reader UX.
- **Content volume**: 20–60 books/year, thousands of historical highlights, recurring desire to revisit and retain what was highlighted.
- **Technical comfort**: Medium. Will import files and follow a guide if the result is durable and private.
- **Platforms**: iPhone and desktop; Obsidian later if he writes about what he reads.
- **Current stack**: Kindle + Readwise + maybe Reader for articles.
- **Pain points**:
  - Highlights are stranded unless synced to a SaaS product.
  - Spaced repetition often feels disconnected from the source material.
  - Books and articles are reviewed in separate systems.
  - He wants his own library of excerpts, not just a daily email.
- **Switching trigger**: A Kindle import flow that produces book-linked highlights, notes, tags, and review cards without depending on a subscription service.
- **What “done” looks like for Jonah**:
  - Imported highlights are organized by book, author, tag, and date.
  - Cards can be generated from highlights with minimal friction.
  - Review history is local, portable, and reusable.
- **Roadmap phase fit**: **Phase 3** is the key inflection point; **Phase 4** keeps it alive on mobile.

### 7. Samira — The Terminal-Native Local-First Builder  

**Contributor and trust persona**

- **Archetype**: Developer/operator who may be a secondary end user but a primary architectural validator.
- **Content volume**: Moderate personal use; high interest in inspecting and extending the system.
- **Technical comfort**: Very high. Rust, SQLite, CLI tools, self-hosting, and open-source contribution are normal.
- **Platforms**: Linux/macOS terminal, maybe self-hosted server.
- **Current stack**: A patchwork of scripts, small tools, and partially trusted SaaS.
- **Pain points**:
  - Many reading tools are impossible to inspect or extend.
  - Export is often lossy or insufficient for reproducibility.
  - “Local-first” often means “cloud-first with an offline cache.”
  - Sync servers tend to become application servers, violating trust boundaries.
- **Switching trigger**: A clean monorepo with a zero-I/O core, transparent schema, predictable exports, strong CLI/TUI ergonomics, and an optional self-hosted sync server that stores only encrypted payloads.
- **What “done” looks like for Samira**:
  - She can trust the architecture, not just the marketing.
  - The system behaves well in scripts, terminals, and backups.
  - She can contribute without disentangling hidden platform logic from the core.
- **Roadmap phase fit**: **Phase 1** validates the foundation; **Phase 4** validates the sync story.

### Persona-to-phase map

| Persona | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 |
|---|---:|---:|---:|---:|---:|
| Nadiya — Feed Maximalist | ✅ Primary | ✅ Important | ⚪ Nice | ✅ Important | ⚪ Nice |
| Marco — Read-Later Triage | ⚪ Partial | ✅ Primary | ✅ Important | ✅ Important | ⚪ Nice |
| Priya — Obsidian Gardener | ⚪ Partial | ⚪ Partial | ✅ Primary | ✅ Important | ✅ Primary |
| Elias — Newsletter Archivist | ⚪ Partial | ✅ Primary | ⚪ Nice | ✅ Important | ⚪ Nice |
| Mina — PDF Researcher | ⚪ Partial | ✅ Primary | ✅ Important | ⚪ Nice | ⚪ Nice |
| Jonah — Kindle Migrant | ⚪ Minimal | ⚪ Minimal | ✅ Primary | ✅ Important | ⚪ Nice |
| Samira — Terminal Builder | ✅ Primary | ✅ Important | ✅ Important | ✅ Important | ⚪ Nice |

---

## Section 2: Cross-Platform Architecture & Core Library Design

### 2.1 Layered architecture (zero-I/O core pattern)

**Status**: [Validated] on the architectural direction; [Validation Required] on the exact crate split below.

pergamon should follow the same hard boundary that makes the rest of the kafkade portfolio maintainable: **the domain core must not perform I/O**. That means no network access, no file reads, no SQLite calls, no platform APIs, and no environment inspection inside `pergamon-core`.

#### Recommended architecture

```text
┌────────────────────────────────────────────────────────────────────┐
│                         Frontend shells                           │
│  CLI commands   ratatui TUI   iOS (SwiftUI)   Web (WASM)         │
└───────────────┬───────────────┬───────────────┬───────────────────┘
                │               │               │
                ▼               ▼               ▼
┌────────────────────────────────────────────────────────────────────┐
│                    Application / use-case layer                   │
│  capture, ingest, triage, annotate, review, search, export       │
│  implemented in pergamon-core as pure orchestration over ports       │
└───────────────────────────────┬────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────┐
│                    pergamon-core (zero I/O domain)                   │
│  Canonical entities                                                │
│  - documents, sources, subscriptions, annotations, review cards    │
│  Pure logic                                                        │
│  - URL normalization, dedup scoring, state machines, SRS, ranking  │
│  Ports / traits                                                    │
│  - document repo, blob repo, sync outbox, clock, id generator      │
└───────────────┬─────────────────────────┬──────────────────────────┘
                │                         │
                ▼                         ▼
┌─────────────────────────────┐  ┌──────────────────────────────────┐
│ Infrastructure adapters     │  │ Parsing / normalization adapters │
│ pergamon-db, pergamon-sync,       │  │ pergamon-ingest (XML/HTML/MIME/PDF) │
│ blob store, sync transport  │  │ takes bytes/strings, returns     │
│                             │  │ canonical normalized structs      │
└───────────────┬─────────────┘  └──────────────────────────────────┘
                │
                ▼
┌────────────────────────────────────────────────────────────────────┐
│                 Platform I/O and optional network                 │
│ filesystem, IMAP, HTTP feed fetch, article download, local files, │
│ keychain access, push scheduling, browser/web bridge              │
└────────────────────────────────────────────────────────────────────┘

Separate deployment target:
┌────────────────────────────────────────────────────────────────────┐
│                    pergamon-server (Axum, AGPL-3.0)                  │
│ Stores encrypted sync batches and blob chunks; never indexes or   │
│ interprets user plaintext documents.                              │
└────────────────────────────────────────────────────────────────────┘
```

#### Hard rules

1. **`pergamon-core` is pure domain and policy.**  
   It may parse/validate already-provided values, but it may not open files, fetch URLs, talk IMAP, or speak SQL.

2. **Parsing can be separate from I/O.**  
   HTML, RSS, Atom, MIME, and PDF parsing should happen in a crate that accepts raw bytes or strings. That crate can still compile to WASM because it does not own transport.

3. **All persistence is behind ports.**  
   The core should depend on repository traits like `DocumentRepo`, `AnnotationRepo`, `CardRepo`, `BlobCatalog`, and `SyncOutbox`. `pergamon-db` implements those traits.

4. **All frontends call the same use cases.**  
   The CLI, TUI, iOS app, and web app should all invoke the same capture, archive, annotate, and review policies. Platform variance should be visual, not semantic.

5. **The sync server is not an app server.**  
   It stores encrypted payloads, device state, and blobs. It does not run article extraction, FTS indexing, or personalized ranking.

#### Why this matters specifically for pergamon

pergamon is not a single-domain CRUD app. It combines:

- streaming-ish ingestion (feeds and mail),
- document normalization (HTML/PDF/newsletter extraction),
- archival storage,
- search,
- annotation,
- spaced repetition,
- and optional sync.

Without strict boundaries, the product will gradually become a CLI wrapper around an increasingly tangled pile of fetchers, parsers, and SQL. The zero-I/O pattern keeps the difficult policy logic—deduplication, queue state, annotation semantics, review scheduling—portable to iOS and web.

#### Recommended domain boundaries inside `pergamon-core`

At minimum, `pergamon-core` should own:

- **Document identity**
  - canonical IDs
  - source provenance
  - external identifiers
  - URL normalization rules
- **Document lifecycle**
  - captured, unread, reading, archived, discarded
  - later vs reference intent
- **Collections and tags**
  - nested collection tree
  - smart/saved queries
- **Annotations**
  - highlight spans, notes, comments, excerpt provenance
- **Review system**
  - reviewable excerpts/cards
  - queues, due dates, interval state, review history
- **Search semantics**
  - query AST, filters, ranking metadata
- **Deduplication policy**
  - exact vs likely duplicate thresholds
  - automatic merge vs manual suggestion

Everything above benefits from pure tests and shared reuse. That is the part that must remain uncompromised.

---

### 2.2 Monorepo layout

**Status**: [Validated] on using a monorepo; [Validation Required] on the exact folder names.

#### Recommended workspace layout

```text
pergamon/
├── Cargo.toml
├── rust-toolchain.toml
├── crates/
│   ├── pergamon-core/        # Zero-I/O domain: entities, policies, SRS, queries
│   ├── pergamon-ingest/      # Pure parsers/normalizers for RSS, Atom, HTML, MIME, PDF text
│   ├── pergamon-db/          # SQLite schema, migrations, repos, FTS5, blob catalog
│   ├── pergamon-sync/        # Sync event model, batch encryption, snapshot logic
│   ├── pergamon-export/      # Markdown/JSON/CSV/OPML export, Obsidian payload shaping
│   ├── pergamon-cli/         # clap v4 binary + ratatui TUI + desktop-side HTTP/IMAP/file I/O
│   ├── pergamon-server/      # Axum sync/blob server (AGPL-3.0)
│   └── pergamon-uniffi/      # UniFFI facade for Apple clients
├── bindings/
│   └── swift/
├── apps/
│   ├── ios/               # SwiftUI app using pergamon-uniffi bindings
│   ├── web/               # WASM web shell + browser storage/sync adapters
│   └── obsidian/          # Obsidian plugin (TypeScript)
├── docs/
│   ├── adr/
│   └── roadmap.md
├── tests/
│   └── fixtures/          # OPML, feeds, HTML, .eml, PDFs, Kindle samples
└── scripts/
```

#### Crate responsibilities

| Component | Responsibility | Why it exists separately |
|---|---|---|
| `pergamon-core` | Canonical domain model and product policy | Must stay portable, testable, and shareable across every client |
| `pergamon-ingest` | Parse raw XML/HTML/MIME/PDF text into normalized structs | Parsing complexity is real, but transport should stay elsewhere |
| `pergamon-db` | SQLite schema, FTS5, migrations, blob catalog, repository impls | Keeps SQL and storage lifecycle out of the core |
| `pergamon-sync` | Event batches, snapshots, encryption, conflict metadata | Sync deserves explicit boundaries and tests |
| `pergamon-export` | Stable external representations: Markdown, JSON, CSV, OPML, Obsidian materialization | Export stability is a product promise |
| `pergamon-cli` | First-class CLI and ratatui TUI; desktop fetch/import runtime | This is the v1 product surface |
| `pergamon-server` | Optional remote store for encrypted sync payloads | Must stay isolated for both license and trust reasons |
| `pergamon-uniffi` | Stable Apple-facing surface over Rust internals | Avoid exposing raw internal crate complexity to Swift |
| `apps/obsidian` | Vault-side integration and affordances | Plugin logic is different enough to justify its own app |

#### Why not fewer crates?

A single crate is tempting in Phase 0, but pergamon is almost guaranteed to need:

- desktop-only runtime dependencies (`reqwest`, IMAP client, filesystem),
- browser-specific storage and WASM shims,
- Apple binding generation,
- server-only sync logic,
- and export logic with long-term compatibility guarantees.

The split above is enough to preserve boundaries without over-fragmenting the codebase.

#### Why not more crates?

You could split `pergamon-core` into smaller subcrates (`pergamon-srs`, `pergamon-query`, `pergamon-domain`), but that is premature. The strongest split is between **portable policy** and **I/O-heavy adapters**. Everything else can wait until compile times or contributor ergonomics justify it.

---

### 2.3 Database architecture (SQLite + FTS5, content storage strategy, storage budget)

**Status**: [Validated] on SQLite+FTS5; [Validation Required] on some schema details.

#### Canonical persistence model

SQLite should be pergamon’s **single canonical local store** on native platforms. It is the right fit because pergamon needs:

- transactional updates across metadata, annotations, and queues,
- strong local search via FTS5,
- portability and inspectability,
- incremental migrations,
- and a small operational footprint.

The database should hold **metadata and normalized text**, while large immutable binaries live in a **content-addressed blob store** on disk.

#### Recommended logical schema

| Table / group | Purpose | Notes |
|---|---|---|
| `sources` | Canonical source records | Feed, newsletter mailbox, manual import, Kindle import, local file |
| `subscriptions` | Ongoing content sources | Feed definitions, polling metadata, mailbox folders, sender rules |
| `documents` | One row per logical saved item/document | Top-level identity used across UI, search, and sync |
| `document_versions` | Materialized content snapshots | Stores normalized text, extraction metadata, content hash, parser version |
| `document_links` | External identifiers and URLs | Canonical URL, original URL, feed GUID, message-id, ISBN/ASIN, etc. |
| `collections` | Nested Raindrop-style folders | Tree structure, ordered manually by user |
| `tags` / `document_tags` | Freeform labels | Many-to-many, case-normalized |
| `annotations` | Highlights, notes, comments | Anchored to document version/page/offset/provenance |
| `review_cards` | SRS cards derived from annotations or manual notes | Card body, prompt/answer, state, origin link |
| `review_events` | Review history | Append-only |
| `saved_queries` | Smart collections / named filters | Serialized query AST or DSL |
| `blob_refs` | Raw HTML/PDF/MIME/blob metadata | SHA-256, mime type, byte size, retention policy |
| `sync_outbox` | Pending outbound mutations | Thin event layer over canonical tables |
| `sync_state` | Device and cursor tracking | Per-device watermarks, snapshot markers |
| `tombstones` | Deleted entities retained for sync | Soft delete with timestamps |
| `fts_documents` | FTS5 virtual table | Indexed text, title, author/source, tag facets |

#### Content storage strategy

**Recommendation**: split storage into three tiers.

1. **Tier A — relational metadata in SQLite**  
   Includes title, authors, source info, tags, collections, read state, due state, sync state, provenance, and lightweight extraction metadata.

2. **Tier B — normalized searchable text in SQLite**  
   Includes article body text, newsletter body text, extracted PDF text, excerpt snippets, and card text. This is what powers FTS5 and cross-platform query semantics.

3. **Tier C — immutable raw blobs on disk**  
   Includes:
   - raw HTML snapshots,
   - original `.eml` source payloads where retained,
   - original PDFs,
   - optional embedded images/assets as needed.

Blobs should be named by content hash, for example:

```text
blobs/
  sha256/
    8a/
      8a7c...f2.html
      8a7c...f2.meta.json
```

This gives deduplication “for free” at the storage layer and makes backup/sync chunking easier.

#### Why normalized text belongs in SQLite

pergamon is not just an archive; it is a **search and review engine**. Search must remain fast, portable, and reliable offline. That means normalized plaintext should be queryable without reopening raw HTML or PDFs.

The largest mistake here would be to store only raw artifacts and defer extraction at read time. That would make:

- search inconsistent,
- mobile/web clients much more expensive,
- and parser upgrades harder to reason about.

#### FTS5 design recommendation

Use **external-content FTS5 tables** rather than stuffing everything into a single denormalized shadow table. Index at least:

- title,
- normalized body text,
- source/publication,
- author/sender,
- tags,
- collection path.

Recommended ranking inputs:

- `bm25()` score from FTS5,
- recency boost,
- explicit “saved/reference” boost,
- reviewability boost for annotated documents.

Ranking policy belongs in `pergamon-core`; index maintenance belongs in `pergamon-db`.

#### Recommended storage budget

pergamon should explicitly design for a power-user local library, not a demo dataset.

##### Desktop target budget

| Asset type | Target volume | Expected storage shape |
|---|---:|---|
| Documents (articles, newsletters, bookmarks with snapshots) | 10,000 | Mostly metadata + normalized text |
| PDFs | 2,000 | Dominated by raw files; extracted text much smaller |
| Annotations / highlights | 100,000 | Small structured rows |
| Review cards | 25,000 | Small structured rows |
| Raw blob budget | ~5 GB managed default | User-adjustable retention policy |

This means pergamon should comfortably handle a serious long-term archive on a laptop without external infrastructure.

##### Mobile/web budget guidance

- **iOS**: default to metadata + recent text + selective cached blobs; aggressively manage old raw assets.
- **Web**: default to metadata + text in browser storage, with optional on-demand blob retention depending on OPFS quota.
- **Server**: store encrypted batches and blob chunks; no search index, no plaintext cache.

#### Retention policy recommendation

Raw blobs should have policy classes:

- `pinned` — never evict automatically
- `keep-original` — retain unless user prunes
- `reconstructable` — safe to re-fetch if needed
- `cache-only` — evictable under budget pressure

That policy matters because not every item deserves permanent raw retention. A saved bookmark with easy re-fetch semantics is not the same as a PDF report or a newsletter issue that may disappear.

---

### 2.4 Platform compilation targets table

**Status**: [Validated] on Rust + UniFFI + WASM; [Validation Required] on the exact web shell implementation.

| Platform | Rust target(s) | Primary UI shell | Local storage backend | Notes | Phase |
|---|---|---|---|---|---|
| Linux CLI/TUI | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | `clap` + `ratatui` | Native SQLite + blob directory | Primary desktop target; first-class product surface | Phase 1 |
| macOS CLI/TUI | `x86_64-apple-darwin`, `aarch64-apple-darwin` | `clap` + `ratatui` | Native SQLite + blob directory | Same feature set as Linux; good early adopter platform | Phase 1 |
| Windows CLI/TUI | `x86_64-pc-windows-msvc` | `clap` + `ratatui` | Native SQLite + blob directory | Required for credibility as a cross-platform personal tool | Phase 1 |
| iOS | `aarch64-apple-ios`, `aarch64-apple-ios-sim` | SwiftUI via UniFFI | Native SQLite + app sandbox blobs | Focus on capture, read, highlight, review, sync | Phase 4 |
| Web | `wasm32-unknown-unknown` | Browser app shell consuming WASM core | Browser SQLite/OPFS or IndexedDB-backed adapter | Must remain local-first; sync is optional, not mandatory | Phase 5 |
| Sync server | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | Axum API | Server-side metadata + encrypted object/blob store | No user plaintext processing | Phase 4 |
| Obsidian plugin | N/A (TypeScript) | Obsidian plugin API | Files in user vault + pergamon sync/export bridge | Separate from core, but crucial for knowledge workflows | Phase 5 |

#### Important platform decision

pergamon should **not** build a native desktop GUI before the CLI/TUI is excellent. The CLI/TUI *is* the desktop app through the early roadmap. That keeps platform count sane and ensures the core interaction model is keyboard-first and scriptable.

---

### 2.5 Sync strategy (following ldgr/tock pattern)

**Status**: [Validated] that pergamon should have an Axum sync server; [Validation Required] on the exact crypto and conflict details.

#### Recommended sync model

pergamon should use the **ldgr/tock pattern**, adapted for content rather than finance/tasks:

- **SQLite is the source of truth.**
- **Sync is a thin replication layer over canonical local state.**
- **Every local mutation writes both canonical tables and an outbox row in the same transaction.**
- **The server stores encrypted batches and blobs but never needs to understand document contents.**

This is the right middle ground between:

- naïve file-copy sync (too fragile),
- full event sourcing (too heavy),
- and cloud-primary REST backends (wrong for local-first).

#### Sync pipeline

1. User edits local state.
2. `pergamon-db` commits:
   - canonical table changes,
   - sync outbox entries,
   - updated entity version metadata.
3. `pergamon-sync` batches pending outbox rows.
4. Batch is compressed and encrypted client-side.
5. Server stores opaque batch and encrypted blob chunks.
6. Other devices fetch unseen batches.
7. Device replays them into local SQLite, resolving conflicts by entity policy.
8. Periodic encrypted snapshots reduce onboarding and replay cost.

#### Recommended server role

`pergamon-server` should do only the following:

- authenticate devices/users,
- accept encrypted event batches,
- store encrypted blobs/chunks,
- serve cursor-based sync streams,
- prune acknowledged data by retention policy.

It should **not**:

- run FTS,
- parse feeds,
- fetch articles,
- render reader mode,
- deduplicate content,
- or generate review cards.

That keeps trust boundaries clean and license boundaries obvious.

#### Conflict policy by entity class

| Entity class | Sync strategy | Conflict policy | Complexity |
|---|---|---|---|
| Feed subscriptions / settings | Mutable config | Last-writer-wins with audit trail | 🟡 |
| Collections / tags | Mostly set-like metadata | Set union where safe; tombstones for deletes | 🟡 |
| Documents (title override, state, intent) | Mutable metadata | Per-field LWW with conflict note if same field changed concurrently | 🔴 |
| Document versions / snapshots | Immutable content revisions | Content-hash dedup; keep both revisions if materially different | 🔴 |
| Annotations | User-authored, sometimes concurrent | Append new annotations automatically; conflicting edits to same annotation body create conflict copy | 🔴 |
| Review cards | Mutable but derivable | Card body conflicts create conflict copy; review history stays append-only | 🔴 |
| Review events | Append-only | Merge automatically | 🟢 |
| Blobs | Immutable by hash | Dedup by hash; no logical conflict | 🟢 |

#### Why a hybrid conflict model is right for pergamon

Finance needs stricter user review. Personal productivity sometimes tolerates aggressive auto-merge. pergamon sits in between.

- If two devices add different tags to the same document, merge automatically.
- If two devices add separate highlights, merge automatically.
- If two devices edit the same note body or card body, **do not** silently choose one; preserve both and surface the conflict.
- If a device changes document state from `reading` to `archived` while another adds a highlight, both should survive.

That produces an experience that is practical without being cavalier.

#### Encryption recommendation

To match the sibling-project trust model:

- device onboarding via public-key exchange,
- sync batches encrypted client-side,
- blob payloads encrypted client-side,
- server stores ciphertext only.

pergamon does **not** need full client-side field encryption inside local SQLite in v1. The product benefits more from excellent local search, indexing, and portability. OS disk encryption plus E2EE sync is the right first compromise for this domain.

#### Snapshot strategy

Use periodic **encrypted snapshots** plus incremental event batches.

- New device onboarding should not require replaying years of article saves one event at a time.
- Snapshots should include enough canonical state to bootstrap quickly:
  - documents,
  - collections/tags,
  - annotations,
  - cards/review state,
  - cursor metadata.

This keeps sync latency acceptable as the library grows.

---

## Section 3: Content Domain Complexity

### 3.1 Content type taxonomy

**Status**: [Validated] on the top-level content set; [Validation Required] on some subtype semantics.

pergamon needs a taxonomy that is broad enough to cover the real inputs, but narrow enough that one library model still makes sense. The goal is not to perfectly model every source’s quirks. The goal is to create a **canonical document graph** that supports search, reading, annotations, and review.

#### Recommended canonical taxonomy

| Canonical type | Typical origins | Identity anchor | Must store | Common derived data | Complexity | Status |
|---|---|---|---|---|---|---|
| **Feed subscription** | RSS/Atom URL, imported OPML | Feed URL + canonicalized feed identity | title, site URL, polling metadata, categories | unread counts, fetch history | 🟡 | [Validated] |
| **Feed entry** | Item from a subscription | feed identity + entry GUID/id/link | title, published time, source feed, summary/content | unread/archive state, extracted article snapshot | 🔴 | [Validation Required] |
| **Saved bookmark** | Manual save, browser share, feed save action | canonical URL | title, URL, collection, tags, intent/state | extracted article, preview, dedup links | 🟡 | [Validated] |
| **Extracted web article** | Saved bookmark, feed entry, direct URL capture | canonical URL + content hash | normalized body, author, site, published date | highlights, notes, cards, offline snapshot | 🔴 | [Validated] |
| **Newsletter issue** | IMAP sync, `.eml` import | `Message-ID` + sender/date fallback | sender, subject, issue date, HTML/text bodies | extracted sections, tags, highlights, cards | 🔴 | [Validated] |
| **PDF document** | Local file import, web PDF capture | blob hash + stable source URL if any | original PDF, extracted text, title metadata | page anchors, highlights, cards | 🔴 | [Validated] |
| **Kindle book highlight set** | My Clippings / exported notebook | book identity + highlight anchors | book metadata, highlight text, location, notes | cards, resurfacing, Obsidian exports | 🟡 | [Validated] |
| **Annotation** | User highlight/note/comment | pergamon ID anchored to document version/page/span | exact excerpt provenance, note body, timestamps | cards, backlinks, review prompts | 🟡 | [Validated] |
| **Review card** | Manual creation or annotation-derived | pergamon ID | prompt, answer/context, due state, interval data | review history, resurfacing analytics | 🔴 | [Validated] |

#### Canonical modeling principle

A bookmark, article, newsletter, PDF, and Kindle excerpt should not become five isolated silos. They should be modeled as:

- a **document** or document-linked entity,
- with a clear source/provenance trail,
- optional raw blobs,
- optional annotations,
- optional review cards.

That makes cross-type workflows possible:

- “show everything tagged `distributed-systems`”
- “review all excerpts from newsletters this month”
- “find PDFs and articles mentioning the same term”
- “surface unread saved items older than 30 days”

#### Three important distinctions

1. **Source vs document**  
   A feed or mailbox is a *source*. An article, issue, or PDF is a *document*. The system must not confuse the two.

2. **Link vs extracted document**  
   A bookmark can exist before extraction. pergamon must allow fast capture even when full extraction is deferred.

3. **Annotation vs card**  
   A highlight is not automatically a good flashcard. Cards are derived, review-focused artifacts with their own lifecycle.

---

### 3.2 Feed parsing complexity

**Status**: [Validated] that feed support is core; [Validation Required] on exact parser behavior and edge-case coverage.

At first glance, RSS/Atom support looks easy. In practice, it is one of the trickier parts of the product because feeds are an old ecosystem with many semi-standard variations.

#### Why feeds are hard

##### 1. Identity is unreliable 🔴

Feeds may expose any of the following for item identity:

- RSS `guid`
- Atom `id`
- permalink `link`
- no stable identifier at all

Some publishers change GUID strategy midstream. Some reuse links with updated tracking parameters. Some rewrite publication dates. Some feed generators emit malformed XML that still “works” in tolerant readers.

**Recommendation**: identity priority should be:

1. explicit feed item ID/GUID if stable,
2. canonicalized permalink,
3. hash of stable fields (`title + published + author + normalized content excerpt`).

Only exact identity matches should auto-merge. Fuzzy matches should produce candidate dedup suggestions, not silent merges.

##### 2. Content payload quality varies wildly 🔴

An entry may contain:

- full HTML in `content:encoded`,
- truncated summary in `description`,
- plain text in Atom `content`,
- or only a link.

Some feeds are effectively full-text article feeds. Others are just teasers that require article fetching. Some embed unreadable boilerplate.

**Recommendation**: store the feed payload as ingested, but treat it as a *source hint*, not guaranteed canonical content. If the user saves an item, pergamon should prefer a full extraction pass using the article URL when available.

##### 3. HTTP polling behavior matters 🟡

Feed ingestion is not just parsing XML; it is also a fetch policy problem:

- ETag and `Last-Modified`
- retry policy
- backoff on failures
- per-feed poll intervals
- disabled feeds
- redirected feed URLs

**Recommendation**: fetch metadata should live in `subscriptions`, not in the parser. Use conditional GETs by default and back off aggressively for failing feeds.

##### 4. Feeds change without warning 🟡

- Feed titles change
- homepages move
- items disappear
- category structures evolve
- feeds flip from HTTP to HTTPS or from one CDN URL to another

pergamon should treat source metadata as mutable and content items as durable. A publisher deleting an old feed entry should **not** delete a user-saved document from the local library.

#### Recommended feed ingestion policy

1. **Subscription is explicit.**  
   Users subscribe to a feed source; entries are ingested into a source-local stream.

2. **Unread state is local state.**  
   It is not derived from feed contents. It lives in pergamon.

3. **Saving creates a document-level commitment.**  
   When a feed item is saved, it becomes a first-class document/bookmark/article in the main library, independent of future feed churn.

4. **Archive does not mean delete.**  
   Feed inbox cleanup is separate from library retention.

5. **OPML is a first-class migration/export format.**  
   Replacing Inoreader without OPML is incomplete.

#### Parser recommendation

Use a tolerant Rust feed parser and normalize into a canonical intermediate structure containing:

- source URL
- item ID/GUID
- canonical link
- title
- author
- published/updated times
- feed categories
- summary HTML/text
- content HTML/text
- enclosure metadata if present

This intermediate representation should then be passed into `pergamon-core` for dedup and state assignment.

#### Failure-handling policy

When a feed is malformed:

- do not crash the sync loop,
- preserve the raw response for debugging when useful,
- mark the subscription with an actionable error state,
- continue processing other subscriptions.

Feed failure is an operational reality, not an exceptional edge case.

---

### 3.3 Content extraction complexity

**Status**: [Validated] that extraction is core to web/newsletter/PDF support; [Validation Required] on exact extraction stack.

Extraction is the part that makes pergamon more than a bookmark database. It is also one of the highest-risk engineering areas because “reader mode” quality directly shapes trust.

#### Recommended extraction philosophy

**Always keep the raw source when it matters, but build the product on normalized content.**

That means:

- articles should retain raw HTML snapshot + normalized text blocks,
- newsletters should retain MIME-derived source + normalized readable form,
- PDFs should retain original bytes + extracted text layer when available.

#### Extraction tiers

| Input type | Recommended extraction path | Complexity | Notes |
|---|---|---|---|
| Web article | fetch raw HTML → snapshot → readability extraction → block normalization | 🔴 | Needs boilerplate stripping, canonical URL handling, metadata heuristics |
| Newsletter | parse MIME → choose best body part → sanitize HTML → article-style normalization | 🔴 | Email HTML is often uglier than web HTML |
| PDF | retain binary → extract text layer and page map → normalize paragraphs | 🔴 | Good enough for text PDFs; OCR deferred |
| Feed full text | normalize feed payload as provisional body | 🟡 | Good fallback when article fetch is unavailable |
| Feed teaser | store summary only until full extraction requested | 🟡 | Must avoid pretending teaser text is full content |

#### Web article extraction

This is harder than “just run Readability” because real-world pages contain:

- cookie overlays,
- paywall stubs,
- inline newsletter signup sections,
- related-content blocks,
- navigation cruft,
- code blocks,
- blockquotes,
- lazy-loaded images,
- and unstable canonical URLs.

**Recommendation**:

- Snapshot the raw HTML as fetched.
- Compute a canonical URL after redirect resolution and normalization.
- Extract a normalized block model:
  - heading
  - paragraph
  - list
  - quote
  - code/pre
  - image caption placeholder
  - horizontal rule / section delimiter
- Store parser version so old documents can be reprocessed later.

The block model matters because annotation anchoring and Obsidian export are easier against normalized blocks than against raw HTML.

#### Newsletter extraction

Newsletter HTML is often worse than article HTML because it is designed for email clients, not browsers. Expect:

- nested tables,
- aggressive inline styles,
- tracking pixels,
- repeated unsubscribe/legal blocks,
- quotation wrappers,
- and sender-specific templates.

**Recommendation**:

- Parse newsletters as documents with strong provenance:
  - sender,
  - subject,
  - received date,
  - `Message-ID`,
  - mailing-list headers if present.
- Use sender-specific cleaning rules **only as additive heuristics**, not as the default architecture.
- Preserve the original email payload hash so reprocessing is possible.

The key here is to model newsletters as **first-class publication issues**, not as weird articles.

#### PDF extraction

PDFs are fundamentally different because they do not contain semantic HTML-like structure. Even good text-layer PDFs often yield:

- broken paragraph boundaries,
- header/footer repetition,
- footnote fragmentation,
- two-column ordering problems,
- and imperfect title metadata.

**Recommendation**:

- v1 supports **text-layer PDFs only**.
- Preserve original PDF bytes always.
- Extract plain text and page offsets.
- Anchor highlights to page number + text offsets + excerpt fingerprint.
- Defer OCR entirely.

This gives useful search and review behavior without pretending PDFs are as structurally clean as HTML.

#### Reprocessing is a product feature

Extraction will improve over time. Therefore pergamon must preserve enough provenance to support safe reprocessing:

- raw blob hash,
- parser version,
- extraction timestamp,
- normalized content hash.

A future parser upgrade should be able to say: “rebuild normalized body for these 200 documents without changing user annotations.”

That requirement should shape the schema from day one.

---

### 3.4 Bookmark & “saved for later” model (Raindrop replacement)

**Status**: [Validated] that collections and tags are essential; [Validation Required] on some state semantics.

pergamon should not treat bookmarks as a second-class compatibility layer. If Raindrop replacement is part of the goal, bookmarks need a strong native model.

#### Core recommendation

A **bookmark is a first-class saved item** with:

- a canonical URL,
- one primary collection,
- many tags,
- an intent (`later` vs `reference`),
- and an optional extracted document snapshot.

This is intentionally not “just save an article.” Some links are kept for future reading; others are kept as references, tools, or landing pages. That difference matters.

#### Recommended entity model

```text
Saved Item
├── canonical URL
├── title / site / preview metadata
├── primary collection_id
├── tags[]
├── intent: later | reference
├── state: inbox | unread | reading | archived | discarded
├── extracted_document_id?   # optional
└── source provenance        # feed save, manual save, share sheet, plugin, etc.
```

#### Why one primary collection is the right choice

Raindrop-like users want collections, but multiple primary folder placements create messy duplication semantics. pergamon should recommend:

- **one primary collection**
- **many tags**
- **smart collections** for alternate views

That keeps the model legible in CLI/TUI, on mobile, and in sync.

#### Recommended state machine

```text
captured
  ↓
inbox
  ├──→ unread/later
  ├──→ reference
  └──→ discarded

unread/later
  └──→ reading
          ├──→ archived
          └──→ discarded

reference
  ├──→ archived
  └──→ discarded
```

A few notes:

- `later` means “I intend to read this.”
- `reference` means “I want this in my library whether or not I fully read it.”
- `archived` means “done for now, keep it.”
- `discarded` means “this was noise or temporary.”

That distinction is critical. Without it, saved links become an undifferentiated blob.

#### Bookmark capture expectations

To be a credible replacement for Raindrop/Reader-style saving, pergamon eventually needs multiple capture paths:

- CLI save from URL
- TUI save from feed item
- iOS share sheet
- browser/Obsidian-assisted capture
- import from existing bookmark exports

But the data model should not depend on any single capture method. A saved item is valid whether it came from a shell command or a mobile share sheet.

#### Relationship to extracted content

Not every bookmark needs immediate extraction.

**Recommendation**:

- capture should be fast and low-friction,
- extraction can be eager for some flows and deferred for others,
- the saved item remains valid even if extraction fails.

This prevents network hiccups or hostile pages from blocking the basic act of saving a link.

#### Smart collections

Collections should be complemented by saved queries such as:

- unread and older than 14 days
- tagged `rust` and not reviewed
- newsletters from a given sender
- PDFs with highlights but no cards
- archived references from one domain

This is how pergamon surpasses “folder + tag” bookmark apps and becomes an actual knowledge operating system.

---

### 3.5 Deduplication & content identity

**Status**: [Validated] that cross-source dedup is required; [Validation Required] on exact thresholds and merge UI.

This is one of pergamon’s most important architectural problems because the same thing can appear through multiple paths:

- an RSS item,
- its canonical web article,
- a manually saved bookmark,
- a newsletter linking to the same URL,
- a PDF mirror of the same paper,
- and notes/highlights referencing it later.

pergamon must not become a duplicate swamp.

#### Recommended identity stack

pergamon should use **layered identity**, in this order:

1. **Source-native identity**  
   Example: feed GUID, Atom ID, email `Message-ID`, Kindle highlight location, imported source row ID.

2. **Canonical locator identity**  
   Example: normalized URL, DOI, ISBN, ASIN.

3. **Content fingerprint identity**  
   Example: SHA-256 of normalized text, raw blob hash, excerpt fingerprint.

4. **Near-duplicate similarity**  
   Example: SimHash/MinHash on normalized text for suggestion only.

#### Canonical URL normalization policy

For web content, pergamon should normalize aggressively but predictably:

- lowercase host
- strip default ports
- remove known tracking params (`utm_*`, `fbclid`, `gclid`, etc.)
- normalize trailing slash policy
- preserve meaningful path/query components
- ignore fragment identifiers unless explicitly content-bearing

This logic belongs in `pergamon-core`, not scattered across frontends.

#### Auto-merge vs suggest-only

**Recommendation**:

- auto-merge only when identity is exact or high-confidence deterministic,
- suggest duplicates when similarity is high but identity is not exact.

Examples:

- same feed GUID on re-import → auto-merge
- same `Message-ID` newsletter → auto-merge
- same canonical URL saved twice → auto-merge into one saved item
- same normalized text hash under different URLs → suggest/merge depending on policy
- similar article titles from same domain with different bodies → suggestion only

Silent false-positive merges are far more damaging than a few extra duplicate suggestions.

#### Per-type identity rules

| Type | Primary identity | Secondary identity | Merge posture |
|---|---|---|---|
| Feed entry | feed ID + entry GUID/id | canonical link | auto-merge when exact |
| Bookmark/article | canonical URL | normalized text hash | auto-merge URL matches; otherwise suggest |
| Newsletter | `Message-ID` | sender + subject + date | auto-merge when exact |
| PDF | raw blob hash | DOI/ISBN/title heuristics | auto-merge hash matches; otherwise suggest |
| Kindle highlights | book identity + location range | excerpt text fingerprint | auto-merge when exact |
| Annotation | pergamon local ID | excerpt fingerprint | no automatic merge across conflicting note bodies |

#### Versioning vs deduplication

A document can be the “same thing” while still having multiple versions.

Examples:

- an article updated after publication,
- a PDF replaced with a new revision,
- a saved page re-fetched with corrected content.

Therefore pergamon should distinguish between:

- **logical document identity**
- **materialized content version**

This is why `documents` and `document_versions` should be separate.

#### User-visible behavior recommendation

Users should be able to understand why pergamon thinks two items are the same. The merge/suggestion UI should explain:

- matching canonical URL,
- matching source ID,
- matching content hash,
- or “high textual similarity.”

Opaque dedup is hard to trust. Transparent dedup becomes a feature.

---

### 3.6 Non-goals & red lines

**Status**: [Validated] for the major exclusions below.

These are not “maybe later” features unless explicitly revisited. They are boundaries that protect the product from becoming incoherent.

#### Product non-goals

- ⛔ **No podcast playback or audio queue management**  
  Kora owns that space. pergamon may index podcast newsletters or transcripts later, but it should not become an audio player.

- ⛔ **No social graph, collaboration, followers, comments, or shared collections**  
  pergamon is personal infrastructure, not a social reading network.

- ⛔ **No hosted inbound email address as a required architecture piece**  
  Newsletter capture should work from user-controlled mailboxes and imports first.

- ⛔ **No OCR pipeline in v1**  
  Text-layer PDFs only. OCR is a separate complexity class and should not delay a coherent release.

- ⛔ **No browser-engine-in-the-app ambition**  
  pergamon should not attempt to become a full browser or email client. It stores normalized readable content and references to originals.

- ⛔ **No DRM circumvention or Kindle cloud scraping**  
  Imports must stay on the right side of legal and ethical boundaries.

- ⛔ **No “AI-first” roadmap dependency**  
  The core product must remain useful offline and without paid model APIs.

- ⛔ **No full two-way Obsidian editing in the first integration**  
  Obsidian should not be allowed to mutate pergamon’s canonical document bodies/annotations until conflict semantics are battle-tested.

- ⛔ **No server-side search/indexing of user plaintext**  
  The sync server stores encrypted payloads; personal search remains on-device.

- ⛔ **No attempt to replace full note-taking apps**  
  pergamon owns ingestion, reading, annotation, and resurfacing. Long-form synthesis can stay in Obsidian or other tools.

#### Architectural red lines

- ⛔ **Do not let `pergamon-core` absorb networking or filesystem APIs.**
- ⛔ **Do not store every raw artifact directly inside SQLite blobs.**
- ⛔ **Do not make sync mandatory for normal use.**
- ⛔ **Do not couple the first Obsidian plugin to undocumented internal schemas.**
- ⛔ **Do not treat bookmarks, articles, PDFs, and highlights as unrelated silos.**

#### What pergamon should be instead

pergamon should be:

- a **personal ingestion engine**,
- a **searchable document library**,
- a **highlight and note substrate**,
- a **review/resurfacing system**,
- and a **local-first bridge into deeper knowledge workflows**.

That scope is already substantial. If pergamon executes it well, it will be more coherent—and more durable—than trying to become every adjacent tool at once.

---

## Section 4: Core Feature Set

**Legend**: 🟢 Straightforward · 🟡 Moderate · 🔴 Hard · ⛔ Blocked

The product should be built around a **single unified personal ingestion library**, not four separate mini-apps glued together. RSS items, read-later articles, bookmarks, PDFs, newsletters, and highlights should all flow into the same content graph, then differ only in their source-specific metadata and downstream workflows.

### 4.1 — Feed Management

| Capability | MVP Scope | Full Scope | Complexity | Validation |
|---|---|---|---|---|
| Subscribe to RSS/Atom feed URL | Paste direct feed URL; validate and save | Bulk subscribe, batch add, import from clipboard/list | 🟢 | [Validated] |
| Website discovery | HTML autodiscovery from a site URL | Multi-feed site chooser + feed preview | 🟢 | [Validated] |
| OPML import/export | Import subscriptions and folders; export current subscriptions | Merge, dedupe, and preserve source metadata/comments where available | 🟢 | [Validated] |
| Feed categories/folders | One folder per subscription | Nested folders, bulk move, color/icon labels | 🟡 | [Validated] |
| Refresh engine | Manual refresh + scheduled polling | Adaptive polling, per-feed intervals, backoff, conditional GET tuning | 🟡 | [Validated] |
| Deduplication | Per-feed GUID/URL dedup | Cross-feed canonical URL dedup and redirect consolidation | 🟡 | [Validated] |
| Feed health monitoring | Last success, last error, failure count | “Feed doctor,” redirect repair, dead-feed detection, auto-pause noisy feeds | 🟡 | [Validated] |
| Feed discovery beyond pasted URLs | None | Search/import from known catalogs or recommendations | 🔴 | [Validation Required] |

**Decision**: the MVP should treat feed management as a **reliability problem**, not a “discovery platform” problem. The critical onboarding win is: paste a feed or OPML, refresh, and trust the results. Anything that smells like social discovery or public recommendation can wait.

**Recommended implementation stance**:

- Use **conditional GET** (`ETag`, `Last-Modified`) from day one.
- Keep health monitoring first-class: every feed row should clearly show `healthy`, `degraded`, or `broken`.
- Do **not** build an Inoreader-style public discovery layer for v1. A local-first product wins by ingesting existing subscriptions cleanly, not by trying to be a destination.

### 4.2 — Reading Experience

| Capability | MVP Scope | Full Scope | Complexity | Validation |
|---|---|---|---|---|
| Article list view | Chronological/unread list with source, title, date, read state | Multi-pane layouts, density modes, custom sort orders, split views | 🟢 | [Validated] |
| Reader mode | Store extracted article text and display clean reading view | Per-site extraction tuning, fallback heuristics, typography profiles | 🟡 | [Validated] |
| Keyboard navigation | Vim-like list/reader navigation, open/archive/star/tag actions | User-remappable keys, macros, command palette, jump history | 🟡 | [Validated] |
| TUI rendering | Clean text/markdown rendering in ratatui | Better HTML tables/lists/quotes/images, footnotes, inline metadata panels | 🔴 | [Validated] |
| Read state management | Mark read/unread, archive, save for later, star | Reading progress, resume position, session history, bulk triage | 🟡 | [Validated] |
| Offline reading | Cached cleaned body text | Full local snapshot with raw HTML/PDF asset preservation | 🟡 | [Validated] |
| Media handling | Links and basic images as optional metadata | Inline image previews where platform supports it | 🔴 | [Validation Required] |

The MVP reader should optimize for **speed, clarity, and keyboard flow**:

1. open item,
2. read clean text,
3. archive/star/tag,
4. move on.

That is enough to replace a surprising amount of Readwise Reader usage.

**Recommendation**: do not attempt a full browser inside the TUI. pergamon should render **normalized article text**, not chase pixel-perfect web fidelity. When extraction fails, the fallback should be explicit: open original URL in system browser, but keep the item and metadata in pergamon.

### 4.3 — Bookmark Management / Raindrop Replacement

| Capability | MVP Scope | Full Scope | Complexity | Validation |
|---|---|---|---|---|
| Save bookmark to inbox | Add URL with title fetch if available | Browser capture helpers, share targets, rich preview pipeline | 🟢 | [Validated] |
| Collections | Manual collections/folders | Nested collections, pinned collections, per-collection rules | 🟡 | [Validated] |
| Tags | Unified free-form tags across all content | Tag aliases, merge/rename, hierarchy emulation via prefixes | 🟡 | [Validated] |
| Inbox workflow | New bookmarks land in inbox until triaged | Rules-based triage, auto-tagging, default collection assignment | 🟡 | [Validated] |
| Archival | Archive/unarchive bookmark items | Local snapshots, broken-link detection, refresh snapshots | 🟡 | [Validated] |
| Full-text search | Search titles, URLs, notes, and extracted text | Ranking by recency, saved queries, related-item suggestions | 🟡 | [Validated] |
| URL normalization | Basic canonical URL matching | Redirect tracing, UTM stripping, duplicate merge suggestions | 🟡 | [Validated] |

**Decision**: pergamon should model bookmarks as **content items first**, not as a separate silo. A bookmark may start as a simple URL, then gain extracted text, highlights, tags, a collection, and an Obsidian note. That lifecycle is the whole point of the product.

**Recommendation**:

- Default all new bookmarks to an **inbox** state.
- Use collections for curation, tags for description, and stars for urgency/importance.
- Avoid Raindrop-style heavy visual theming as a v1 priority; terminal-first users need fast triage and dependable metadata far more than card layouts.

### 4.4 — Spaced Repetition & Resurfacing

| Capability | MVP Scope | Full Scope | Complexity | Validation |
|---|---|---|---|---|
| Highlight selection | User explicitly promotes a highlight/quote to reviewable state | Rules that auto-promote tagged highlights, per-source policies | 🟡 | [Validated] |
| Daily review queue | One deterministic daily queue | Queue caps, source balancing, overdue prioritization, custom decks | 🟡 | [Validated] |
| SR algorithm | **FSRS only** | Tunable retention targets, per-user parameter optimization | 🔴 | [Validated] |
| Review actions | Again / Hard / Good / Easy | Suspend, bury, snooze, archive, convert to evergreen note | 🟡 | [Validated] |
| Highlight card types | Basic quote recall / recognition prompt | Cloze, reverse cards, concept prompts, note-backed cards | 🔴 | [Validation Required] |
| Kindle integration | Import Kindle clippings as annotations and optional cards | Location-aware merging, sync with matching books/PDFs/articles | 🟡 | [Validated] |
| Review stats | Due today, completed today, retention, streak | Load forecast, source breakdown, card maturity, “worth keeping?” metrics | 🟡 | [Validated] |

**Decision**: use **FSRS** as the long-term scheduler and do not ship SM-2 as a “starter algorithm.” SM-2 would create migration debt for a feature area where users care deeply about trust and consistency.

**Important scope cut**: not every highlight should become a card. Readwise-style automatic resurfacing often becomes noise when the highlight stream is too broad. pergamon should make the default posture:

- highlights are stored,
- highlights can be searched and exported,
- only **selected highlights** become FSRS cards.

That keeps the review queue meaningful.

**Recommended review model**:

- `highlight` → optionally `review_card`
- one daily queue generated from due cards
- card actions stored as immutable review logs
- card state stored separately as current FSRS state
- stats computed from logs, not ad hoc counters

### 4.5 — Search & Discovery

| Capability | MVP Scope | Full Scope | Complexity | Validation |
|---|---|---|---|---|
| FTS5 full-text search | Search title, excerpt, body text, annotation text | Weighted ranking, source boosts, highlight-aware ranking | 🟢 | [Validated] |
| Fuzzy search | None | Typo-tolerant title/tag/source matching | 🟡 | [Validated] |
| Faceted filtering | Filter by content type, source, tag, collection, star, archive state | Compound facets, query chips, negation, saved facet presets | 🟡 | [Validated] |
| Saved searches | Named query strings | Pinned searches, search-based dashboards, exportable query packs | 🟡 | [Validated] |
| Search syntax | Simple query string + filters | Unified query language across CLI, TUI, export, and smart collections | 🟡 | [Validated] |
| Discovery / related items | None | “Related content,” resurfacing by tag/link similarity, source clustering | 🔴 | [Validation Required] |

**Decision**: pergamon should treat search as a **core product surface**, not a utility feature. Replacing Readwise and Raindrop requires the user to trust that anything they saved can be found later.

**Recommendation**:

- Use **SQLite FTS5** for the canonical index.
- Keep fuzzy matching outside the core index path if needed, so the system stays portable across desktop, iOS, and WASM builds.
- Make the query language reusable for smart collections, export filters, and Obsidian sync selection.

### 4.6 — Content Organization

| Capability | MVP Scope | Full Scope | Complexity | Validation |
|---|---|---|---|---|
| Unified tags | One tag system across feeds, bookmarks, PDFs, newsletters, highlights | Aliases, merge/rename, optional namespacing (`topic/ai`, `source/blog`) | 🟢 | [Validated] |
| Collections | Manual collections for curated grouping | Nested collections, colored collections, drag ordering | 🟡 | [Validated] |
| Smart collections | None | Query-backed collections driven by saved filters | 🟡 | [Validated] |
| Stars | Single-star flag | Multiple pin levels rejected; keep star simple | 🟢 | [Validated] |
| Archive | Archive/unarchive any content item | Auto-archive policies, archival reports, “cold storage” views | 🟢 | [Validated] |
| Bulk organization | Batch tag/add/remove/archive via query | Rules engine, macros, ingestion-time filing | 🟡 | [Validated] |

**Decision**: pergamon should have exactly four universal organization primitives:

- **tags** for meaning,
- **collections** for curation,
- **star** for importance,
- **archive** for lifecycle.

Anything else should be implemented on top of those primitives or deferred.

That restraint matters. A local-first tool becomes unmaintainable if it grows five overlapping classification systems.

### 4.7 — PDF Support

| Capability | MVP Scope | Full Scope | Complexity | Validation |
|---|---|---|---|---|
| PDF import | Import local PDF into library with metadata shell | Watched folders, bulk import, duplicate detection by hash | 🟢 | [Validated] |
| Text extraction | Extract searchable text when possible | Better layout reconstruction, page-local anchors, OCR pipeline | 🔴 | [Validated] |
| Metadata extraction | Title/author/pages from embedded metadata if present | DOI/ISBN detection, heuristic metadata cleanup, external enrichment | 🟡 | [Validated] |
| PDF reading flow | Open metadata + extracted text in pergamon; open original PDF externally | Native/mobile viewers, page thumbnails, inline page jump | 🔴 | [Validation Required] |
| PDF highlights/annotations | Manual note/highlight records attached to PDF | Import external PDF annotations where format permits | 🔴 | [Validation Required] |
| Kindle Clippings | Parse `My Clippings.txt` and attach highlights to matching content | Better matching against books/PDFs/articles and location reconciliation | 🟡 | [Validated] |

**Decision**: PDF support should be **knowledge-management-first**, not viewer-first. The MVP requirement is: import a PDF, extract text if possible, search it, tag it, annotate it, and export it. Building a full cross-platform PDF reading UI can wait.

**Recommendation**:

- Keep binary PDF files as content-addressed assets on disk.
- Store extracted text and normalized metadata in SQLite.
- Treat Kindle clippings as an **annotation import pipeline**, not as a separate content system.

---

## Section 5: Obsidian Integration

The Obsidian integration should exist to make pergamon a better **capture and resurfacing engine**, not to turn Obsidian into a second database. pergamon remains the source of truth for ingestion, organization, and review state; Obsidian becomes the user’s preferred long-form note environment.

### 5.1 — Integration Architecture

**Recommendation**: use **direct read-only SQLite access from the Obsidian plugin** as the primary transport.

This is the simplest architecture because it avoids:

- a local HTTP server,
- a background sync daemon,
- custom IPC layers,
- duplicated export databases,
- or plugin-side parsing of many intermediary JSON files.

**Proposed architecture**:

1. **pergamon owns the canonical SQLite database**.
2. The **Obsidian desktop plugin opens that database in read-only mode**.
3. The plugin queries a small, stable set of **read-optimized SQL views** (for example `obsidian_export_items`, `obsidian_export_annotations`, `obsidian_export_collections`).
4. The plugin materializes or refreshes markdown notes in the vault using configurable templates.
5. Any reverse flow happens only through a **manual pull-back command**, never live bidirectional mutation.

**Why this is the right choice**:

- It preserves **local-first simplicity**.
- It lets Obsidian reflect live pergamon state without inventing another sync protocol.
- It keeps the business logic in pergamon rather than re-implementing it in TypeScript.
- It fits the solo-maintainer reality: fewer moving parts, fewer support failures.

**Constraint note**:

- **Desktop Obsidian**: this approach is strong and practical. [Validated]
- **Mobile Obsidian plugin parity**: weaker and likely partial; mobile can rely on generated markdown notes rather than live DB access. [Validation Required]

**Implementation detail**: the plugin should never write directly into pergamon’s operational tables. If pull-back is supported, it should write to markdown files and let pergamon import from those files on explicit user action.

### 5.2 — Sync Scope

**Recommendation**: sync only **durable knowledge artifacts**, not the entire transient reading backlog.

| Domain | Default Sync | Notes |
|---|---|---|
| Starred content items | Yes | High-signal material belongs in the vault |
| Annotated items | Yes | Highlights/notes are the core knowledge payload |
| Archived long-form items | Yes | Good default for “kept” articles and PDFs |
| Inbox/unread feed firehose | No | Avoid vault spam and note churn |
| Tags and collections | Yes | Needed for navigation and backlinking inside Obsidian |
| Full article/PDF extracted text | Optional | Enable per-template or per-collection; large by default |
| Review card metadata | Partial | Sync card state summary, not the entire review log |
| Raw HTML snapshots / binary caches | No | Vault should not mirror ingestion storage internals |
| Import logs / fetch history / FTS indexes | No | Operational noise, not user knowledge |

This is the key product decision: **Obsidian is for curated knowledge, not for unread triage**.

A good default sync profile would be:

- all items with at least one highlight or note,
- all starred items,
- all items in explicit “Obsidian/*” collections,
- optionally all archived PDFs.

### 5.3 — Note Format

**Recommendation**: markdown files with **YAML frontmatter** plus a stable section structure and configurable Handlebars-style templates.

**Default note structure**:

```markdown
---
folio_id: 01J8W8J7NQ5N3Q2K6T3P4S9V1A
content_type: article
title: "The Shape of Durable Software"
source_title: "Example Blog"
source_url: "https://example.com/durable-software"
canonical_url: "https://example.com/durable-software"
author: ["Jane Example"]
published_at: 2026-02-14T10:00:00Z
added_at: 2026-02-15T08:11:00Z
tags: ["software", "architecture"]
collections: ["Essays", "Obsidian/Inbox"]
starred: true
archived: true
review_state: active
highlight_count: 6
---

# {{title}}

> [!info]
> Source: [{{source_title}}]({{canonical_url}})
> Added: {{added_at}}
> Type: {{content_type}}

## Summary

{{summary}}

## Highlights

- {{highlight_1}}
- {{highlight_2}}

## Notes

<!-- pergamon:user-start -->
User-written notes live here.
<!-- pergamon:user-end -->
```

**Design rules**:

- `folio_id` is mandatory and immutable.
- Frontmatter should contain only **stable metadata**, not volatile UI state.
- The body should separate:
  - source-derived content,
  - pergamon-derived highlights,
  - user-editable note space.

**Template system**:

- Support configurable folder template, filename template, and body template.
- Use simple placeholders like `{{title}}`, `{{tags}}`, `{{highlights}}`.
- Do not expose arbitrary SQL templating in v1.
- Ship a few opinionated presets:
  - `article-note`
  - `pdf-research-note`
  - `quote-only`
  - `daily-review-export`

### 5.4 — Sync Direction

**Recommendation**: make the integration **one-way push from pergamon to Obsidian**, with an explicit **manual pull-back** command.

That means:

- **pergamon is authoritative for**:
  - canonical metadata,
  - source URLs,
  - tags/collections if managed in pergamon,
  - read/archive/star state,
  - highlight storage,
  - FSRS review state.

- **Obsidian is user-authoritative only for**:
  - long-form commentary,
  - synthesis,
  - backlinks,
  - hand-written notes inside designated user blocks.

**Manual pull-back policy**:

- Triggered by command, not background watched sync.
- Pull only from known notes containing `folio_id`.
- Import only allowed fields:
  - note body inside `pergamon:user-start` markers,
  - optional appended tags in a dedicated `obsidian_tags` field,
  - optional edited summary block if explicitly enabled.
- Never let Obsidian silently overwrite core pergamon metadata.

This keeps the system understandable:

- pergamon ingests and organizes.
- Obsidian synthesizes.
- Pull-back is possible, but never magical.

---

## Section 6: Features the User May Have Missed

The phases below assume a rough progression:

- **Phase 1**: ingestion MVP
- **Phase 2**: unified library + bookmarks
- **Phase 3**: daily-driver reader + search
- **Phase 4**: knowledge workflows + Obsidian + SR
- **Phase 5**: ecosystem polish + cross-platform capture

| # | Feature | Why It Matters | Phase | Complexity |
|---|---|---|---|---|
| 1 | **Rules-based auto-filing** | “Anything from arXiv goes to `Research`, tag `paper`.” This removes inbox drag immediately. | Phase 2 | 🟡 |
| 2 | **Canonical URL normalization** | Merges `utm_*`, mobile URLs, and redirects so bookmarks and feed entries stop duplicating each other. | Phase 2 | 🟡 |
| 3 | **Offline snapshot capture** | Keeps a readable local copy when sites disappear, paywall, or rot. Critical for long-term ownership. | Phase 2 | 🟡 |
| 4 | **Feed doctor** | Detect dead feeds, redirect loops, invalid XML, auth failures, and noisy subscriptions. | Phase 2 | 🟡 |
| 5 | **Digest mode** | A daily or weekly “what’s worth reading?” queue is higher value than a giant unread count. | Phase 3 | 🟢 |
| 6 | **Snooze / resurface later** | Not every saved item belongs in FSRS; sometimes “show me this again in 10 days” is enough. | Phase 3 | 🟢 |
| 7 | **Reading heatmap / streaks** | Useful motivation and a clear replacement for Reader-style reading stats. | Phase 3 | 🟡 |
| 8 | **Batch actions by query** | `pergamon tag add ai --query 'source:hn unread:true'` is core CLI power-user value. | Phase 3 | 🟢 |
| 9 | **Annotation promotion pipeline** | Turn a highlight into a card, note, collection item, or Obsidian export target in one action. | Phase 4 | 🟡 |
| 10 | **Duplicate highlight consolidation** | Merge the same quote imported from Kindle, web article, and PDF into one knowledge object. | Phase 4 | 🟡 |
| 11 | **Dead-link / content rot audit** | Periodically check whether original URLs still resolve and whether snapshots are missing. | Phase 4 | 🟡 |
| 12 | **Related-item graph** | Show “more items like this” from shared tags, links, feeds, or recurring highlights. | Phase 4 | 🟡 |
| 13 | **Local web clipper / share target** | Browser helper or OS share extension makes capture frictionless without requiring a hosted service. | Phase 5 | 🟡 |
| 14 | **`pergamon doctor` integrity checks** | Find orphaned assets, broken FTS indexes, duplicate URLs, and failed imports. | Phase 2 | 🟢 |
| 15 | **Quiet hours / source throttling** | Mute noisy feeds or batch refresh low-priority sources so the inbox stays sane. | Phase 5 | 🟡 |

These are not “nice extras.” Several of them are exactly the kinds of small-but-sticky features that turn a personal tool into a daily habit.

---

## Section 7: Moonshot Features

Moonshots should remain **strictly optional** and should never distort the core product: offline ingestion, durable storage, search, resurfacing, and export. If a moonshot threatens the simplicity of the base system, it should be rejected.

### 7.1 — AI Features (BYOK) 🔴

**Recommendation**: support AI only as a **BYOK / BYOM** capability:

- **Bring Your Own Key** for cloud APIs,
- **Bring Your Own Model** for local runtimes like Ollama or LM Studio.

pergamon should not operate a hosted inference service.

**Good AI use cases for pergamon**:

- article summarization,
- suggested tags,
- highlight extraction suggestions,
- semantic “related content,”
- question answering over the local library,
- digest prioritization,
- turning highlights into draft flashcards.

**Bad AI use cases for pergamon**:

- opaque automatic rewriting of source material,
- silent metadata mutation,
- hosted recommendation feeds,
- anything that makes export or reproducibility worse.

**Architecture recommendation**:

- Put AI behind a separate crate, e.g. `pergamon-ai`.
- Keep it outside the zero-I/O core.
- AI outputs must be stored as **derived artifacts with provenance**, including:
  - model/provider,
  - prompt template version,
  - generation timestamp,
  - user approval state.

**Product rule**: AI suggestions are never authoritative until accepted by the user.

That means:

- suggested tags are suggestions,
- suggested highlights are suggestions,
- summaries are derived notes,
- generated flashcards are drafts.

**Complexity drivers**:

- prompt/version management,
- privacy guarantees across providers,
- cost control,
- local model performance on desktop vs mobile,
- semantic index portability across SQLite/WASM/iOS.

**Validation status**:

- BYOK plumbing itself: [Validated]
- High-quality on-device semantic workflows across all platforms: [Validation Required]

**Phase**: post-1.0 research, likely **Phase 6+**.

### 7.2 — Email Newsletter Ingestion 🔴

This is a high-value moonshot because newsletters are now a major reading source, but it is materially harder than RSS.

**Recommendation**: phase it in this order:

1. **`.eml` and Maildir import first** — local, explicit, scriptable. [Validated]
2. **Read-only IMAP folder sync second** — practical, but needs provider-specific testing. [Validation Required]
3. **No hosted forwarding address** — rejected.

That last point matters. A hosted ingestion mailbox would drag pergamon toward a cloud service business. It is the wrong fit.

**Canonical model**:

- each newsletter issue becomes a `content_item` of type `newsletter`,
- raw MIME is optional asset storage,
- cleaned body text is indexed like any other long-form content,
- message identifiers (`Message-ID`, `List-ID`) power deduplication.

**Key technical challenges**:

- malformed MIME,
- tracker/link rewriting,
- multipart HTML/plaintext selection,
- giant quoted reply chains,
- weird publisher templates,
- login/app-password complexity for IMAP.

**User experience rule**:

- newsletters should behave like articles after ingestion,
- but retain newsletter-specific metadata (`sender`, `issue date`, `list id`).

That preserves the value of the source without creating a parallel UX.

**Phase**: likely **Phase 5 or 6**, after the core RSS/bookmark/article pipeline is stable.

### 7.3 — Self-Hosted Web UI 🔴

**Recommendation**: build the self-hosted web UI as a **single-user companion to the Axum sync server**, not as a multi-tenant SaaS clone.

This matches the project DNA:

- local-first,
- open source,
- CLI/TUI-first,
- self-hostable if desired.

**Core shape**:

- Axum provides authenticated API endpoints and static asset serving.
- The web frontend is a WASM or server-rendered hybrid that consumes the same domain model as the native apps.
- The deployment target is a small Docker image for personal hosting.

**What the web UI should do**:

- browse/search library,
- read saved content,
- manage tags/collections,
- review highlights/cards,
- push/export to Obsidian.

**What it should not do in v1**:

- become the primary product surface,
- support multi-user organizations,
- chase social or shared libraries,
- depend on cloud-managed auth.

**Licensing note**:

- this belongs naturally beside the **AGPL Axum server**, not inside the Apache-only core/app crates.

**Why this is moonshot, not core**:

- browser auth/session management,
- server deployment burden,
- sync and access-control edge cases,
- a second major UI surface to maintain.

If pergamon is already winning in CLI/TUI + local DB mode, the self-hosted web UI becomes a multiplier. If not, it becomes a distraction.

**Phase**: **Phase 6+**.

### 7.4 — Cross-App Integration (kora, toku, tock) 🔴

**Recommendation**: integrate the Kafkade tools through **documented handoff contracts and deep links**, not shared databases.

That means:

- common stable IDs,
- `kafkade://`-style deep links,
- small JSON export/import payloads,
- optional CLI bridge commands.

Direct DB coupling would make all four apps harder to evolve independently.

| App | Best Integration | Why It Matters | Phase | Complexity | Validation |
|---|---|---|---|---|---|
| **tock** | Create tasks/reminders from content; link tasks back to pergamon items | “Read this later,” “follow up,” and “turn this into a project” are natural flows | Phase 5 | 🟡 | [Validated] |
| **toku** | Share book metadata, PDFs, Kindle highlights, and reading notes | Books and long-form documents overlap heavily with pergamon’s PDF/highlight layer | Phase 6 | 🟡 | [Validated] |
| **kora** | Ingest podcast episode metadata/transcripts/highlights; deep-link audio playback | Podcasts are a natural adjacent source of information and annotations | Phase 6 | 🔴 | [Validation Required] |

**Detailed direction**:

#### pergamon ↔ tock

This is the clearest early integration.

- `tock add --from pergamon:<id>` should create a task linked to saved content.
- pergamon can expose “Create task” actions for any item.
- Daily review items could optionally generate tasks rather than cards.

This fits both products without over-coupling them.

#### pergamon ↔ toku

Toku owns the **book domain**; pergamon owns **information capture**.
The right split is:

- Toku tracks editions, reading progress, shelves, and book metadata.
- pergamon stores article excerpts, PDFs, clippings, quotes, and research highlights.

A good integration would let a Toku book record link to:

- imported Kindle clippings in pergamon,
- PDF notes associated with the book,
- Obsidian notes derived from those highlights.

#### pergamon ↔ kora

This is more speculative but potentially powerful.
If kora eventually supports podcasts robustly, pergamon could ingest:

- episode metadata,
- transcripts,
- bookmarked timestamps,
- quote/highlight snippets.

That would turn pergamon into the knowledge layer for spoken content without forcing kora to become a note-taking app.

**Rule**: every cross-app feature should still work if the sibling app is absent. Integrations must be additive, never foundational.

---

## Section 8: Data Model

### 8.1 — Unified Content Model Decision

**Recommendation**: use a single `content_items` table as the canonical entity store, with a `content_type` discriminator and **type-specific extension tables** for source-specific fields.

This is the right model because it preserves a unified UX:

- one inbox,
- one search system,
- one tag/collection model,
- one archive/star lifecycle,
- one highlight/review pipeline.

The extension tables prevent the base table from becoming a sparse mess.

**Canonical content types**:

- `article`
- `bookmark`
- `pdf`
- `newsletter`

A feed item is usually an `article` with a `feed_entries` extension row.  
A saved URL may be a `bookmark` even if it later gains extracted text.  
A PDF is a `pdf` plus an asset row.  
A newsletter issue is a `newsletter` plus email-specific metadata.

### 8.2 — Entity Relationship Diagram (Text)

```text
feed_folders (1) ──── (M) feed_subscriptions
feed_subscriptions (1) ──── (M) feed_fetch_runs
feed_subscriptions (1) ──── (M) feed_entries ──── (1) content_items
content_items (1) ──── (1) content_bodies
content_items (1) ──── (0..1) bookmark_details
content_items (1) ──── (0..1) pdf_documents ──── (1) assets
content_items (1) ──── (0..1) newsletter_issues
content_items (1) ──── (M) external_refs
content_items (1) ──── (M) content_field_provenance
content_items (1) ──── (M) annotations
annotations (0..1) ──── (1) review_cards ──── (1) review_state
review_cards (1) ──── (M) review_logs

content_items (M) ──── (M) tags via content_tags
content_items (M) ──── (M) collections via collection_items
saved_searches (standalone)

import_runs (1) ──── (M) import_run_items ──── (0..1) content_items
content_items (1) ──── (0..1) obsidian_sync_state

content_fts (denormalized FTS5 index over content_items + content_bodies + annotations + tags)
```

### 8.3 — Core Tables (SQLite)

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE feed_folders (
  id TEXT PRIMARY KEY, -- UUIDv7
  name TEXT NOT NULL UNIQUE,
  position INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE content_items (
  id TEXT PRIMARY KEY, -- UUIDv7
  content_type TEXT NOT NULL
    CHECK (content_type IN ('article', 'bookmark', 'pdf', 'newsletter')),
  title TEXT NOT NULL,
  canonical_url TEXT,
  source_title TEXT,
  author TEXT,
  excerpt TEXT,
  language TEXT,
  status TEXT NOT NULL DEFAULT 'inbox'
    CHECK (status IN ('inbox', 'saved', 'archived', 'deleted')),
  read_state TEXT NOT NULL DEFAULT 'unread'
    CHECK (read_state IN ('unread', 'reading', 'read')),
  is_starred INTEGER NOT NULL DEFAULT 0
    CHECK (is_starred IN (0, 1)),
  published_at TEXT,
  added_at TEXT NOT NULL,
  opened_at TEXT,
  read_at TEXT,
  archived_at TEXT,
  word_count INTEGER,
  estimated_read_minutes INTEGER,
  content_hash TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);

CREATE TABLE assets (
  id TEXT PRIMARY KEY,
  sha256 TEXT NOT NULL UNIQUE,
  mime_type TEXT NOT NULL,
  storage_rel_path TEXT NOT NULL UNIQUE,
  size_bytes INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE content_bodies (
  content_item_id TEXT PRIMARY KEY
    REFERENCES content_items(id) ON DELETE CASCADE,
  raw_asset_id TEXT
    REFERENCES assets(id) ON DELETE SET NULL,
  body_markdown TEXT,
  body_text TEXT,
  extractor TEXT,
  extractor_version TEXT,
  extracted_at TEXT,
  last_refreshed_at TEXT
);

CREATE TABLE feed_subscriptions (
  id TEXT PRIMARY KEY,
  feed_folder_id TEXT
    REFERENCES feed_folders(id) ON DELETE SET NULL,
  feed_url TEXT NOT NULL UNIQUE,
  site_url TEXT,
  title TEXT NOT NULL,
  description TEXT,
  language TEXT,
  sort_title TEXT,
  icon_url TEXT,
  etag TEXT,
  last_modified TEXT,
  refresh_interval_minutes INTEGER NOT NULL DEFAULT 60,
  paused INTEGER NOT NULL DEFAULT 0
    CHECK (paused IN (0, 1)),
  last_polled_at TEXT,
  last_success_at TEXT,
  last_error_at TEXT,
  consecutive_failures INTEGER NOT NULL DEFAULT 0,
  last_http_status INTEGER,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE feed_fetch_runs (
  id TEXT PRIMARY KEY,
  feed_subscription_id TEXT NOT NULL
    REFERENCES feed_subscriptions(id) ON DELETE CASCADE,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  http_status INTEGER,
  items_seen INTEGER NOT NULL DEFAULT 0,
  items_added INTEGER NOT NULL DEFAULT 0,
  items_updated INTEGER NOT NULL DEFAULT 0,
  error_text TEXT
);

CREATE TABLE feed_entries (
  content_item_id TEXT PRIMARY KEY
    REFERENCES content_items(id) ON DELETE CASCADE,
  feed_subscription_id TEXT NOT NULL
    REFERENCES feed_subscriptions(id) ON DELETE CASCADE,
  entry_fingerprint TEXT NOT NULL,
  guid TEXT,
  comments_url TEXT,
  fetched_at TEXT NOT NULL,
  UNIQUE (feed_subscription_id, entry_fingerprint)
);

CREATE TABLE bookmark_details (
  content_item_id TEXT PRIMARY KEY
    REFERENCES content_items(id) ON DELETE CASCADE,
  original_url TEXT NOT NULL,
  resolved_url TEXT,
  domain TEXT,
  saved_via TEXT, -- cli, browser_extension, import, share_sheet
  snapshot_asset_id TEXT
    REFERENCES assets(id) ON DELETE SET NULL,
  reading_view_ready INTEGER NOT NULL DEFAULT 0
    CHECK (reading_view_ready IN (0, 1))
);

CREATE TABLE pdf_documents (
  content_item_id TEXT PRIMARY KEY
    REFERENCES content_items(id) ON DELETE CASCADE,
  file_asset_id TEXT NOT NULL
    REFERENCES assets(id) ON DELETE RESTRICT,
  file_name TEXT NOT NULL,
  page_count INTEGER,
  doi TEXT,
  isbn TEXT,
  metadata_json TEXT,
  text_extraction_status TEXT NOT NULL DEFAULT 'pending'
    CHECK (text_extraction_status IN ('pending', 'complete', 'failed')),
  imported_from_path TEXT
);

CREATE TABLE newsletter_issues (
  content_item_id TEXT PRIMARY KEY
    REFERENCES content_items(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL UNIQUE,
  list_id TEXT,
  from_name TEXT,
  from_email TEXT,
  subject_line TEXT NOT NULL,
  received_at TEXT NOT NULL
);

CREATE TABLE external_refs (
  id TEXT PRIMARY KEY,
  content_item_id TEXT NOT NULL
    REFERENCES content_items(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,      -- rss, inoreader, raindrop, readwise, kindle, obsidian, manual
  external_id TEXT NOT NULL,
  external_url TEXT,
  imported_at TEXT NOT NULL,
  raw_ref_json TEXT,
  UNIQUE (provider, external_id)
);

CREATE TABLE content_field_provenance (
  content_item_id TEXT NOT NULL
    REFERENCES content_items(id) ON DELETE CASCADE,
  field_name TEXT NOT NULL,
  source_provider TEXT NOT NULL,
  source_reference TEXT,
  recorded_at TEXT NOT NULL,
  is_user_override INTEGER NOT NULL DEFAULT 0
    CHECK (is_user_override IN (0, 1)),
  PRIMARY KEY (content_item_id, field_name)
);

CREATE TABLE annotations (
  id TEXT PRIMARY KEY,
  content_item_id TEXT NOT NULL
    REFERENCES content_items(id) ON DELETE CASCADE,
  annotation_type TEXT NOT NULL
    CHECK (annotation_type IN ('highlight', 'note', 'quote')),
  quote_text TEXT,
  note_markdown TEXT,
  color TEXT,
  page_number INTEGER,
  location_label TEXT,
  locator_type TEXT, -- css_selector, text_quote, pdf_page, kindle_location, generic
  locator_json TEXT,
  source_provider TEXT,
  source_external_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE review_cards (
  id TEXT PRIMARY KEY,
  content_item_id TEXT NOT NULL
    REFERENCES content_items(id) ON DELETE CASCADE,
  annotation_id TEXT
    REFERENCES annotations(id) ON DELETE SET NULL,
  card_type TEXT NOT NULL DEFAULT 'highlight'
    CHECK (card_type IN ('highlight', 'quote', 'concept')),
  prompt_text TEXT,
  answer_text TEXT,
  state TEXT NOT NULL DEFAULT 'active'
    CHECK (state IN ('active', 'suspended', 'archived')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE review_state (
  card_id TEXT PRIMARY KEY
    REFERENCES review_cards(id) ON DELETE CASCADE,
  algorithm TEXT NOT NULL DEFAULT 'fsrs'
    CHECK (algorithm IN ('fsrs')),
  desired_retention REAL NOT NULL DEFAULT 0.90,
  difficulty REAL,
  stability REAL,
  retrievability REAL,
  elapsed_days INTEGER NOT NULL DEFAULT 0,
  scheduled_days INTEGER NOT NULL DEFAULT 0,
  reps INTEGER NOT NULL DEFAULT 0,
  lapses INTEGER NOT NULL DEFAULT 0,
  scheduled_for TEXT,
  last_reviewed_at TEXT,
  suspended INTEGER NOT NULL DEFAULT 0
    CHECK (suspended IN (0, 1))
);

CREATE TABLE review_logs (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL
    REFERENCES review_cards(id) ON DELETE CASCADE,
  reviewed_at TEXT NOT NULL,
  rating TEXT NOT NULL
    CHECK (rating IN ('again', 'hard', 'good', 'easy')),
  elapsed_days INTEGER,
  scheduled_days INTEGER,
  difficulty_before REAL,
  stability_before REAL,
  difficulty_after REAL,
  stability_after REAL
);

CREATE TABLE tags (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  normalized_name TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);

CREATE TABLE content_tags (
  content_item_id TEXT NOT NULL
    REFERENCES content_items(id) ON DELETE CASCADE,
  tag_id TEXT NOT NULL
    REFERENCES tags(id) ON DELETE CASCADE,
  created_at TEXT NOT NULL,
  PRIMARY KEY (content_item_id, tag_id)
);

CREATE TABLE collections (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  collection_type TEXT NOT NULL DEFAULT 'manual'
    CHECK (collection_type IN ('manual', 'smart')),
  description TEXT,
  color TEXT,
  icon TEXT,
  query_json TEXT, -- used when collection_type = 'smart'
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE collection_items (
  collection_id TEXT NOT NULL
    REFERENCES collections(id) ON DELETE CASCADE,
  content_item_id TEXT NOT NULL
    REFERENCES content_items(id) ON DELETE CASCADE,
  position INTEGER,
  added_at TEXT NOT NULL,
  PRIMARY KEY (collection_id, content_item_id)
);

CREATE TABLE saved_searches (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  query_text TEXT NOT NULL,
  is_default INTEGER NOT NULL DEFAULT 0
    CHECK (is_default IN (0, 1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE import_runs (
  id TEXT PRIMARY KEY,
  source_kind TEXT NOT NULL,  -- opml, inoreader, raindrop, readwise, readwise_reader, kindle, pdf, eml
  source_label TEXT NOT NULL,
  source_locator TEXT,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  mode TEXT NOT NULL DEFAULT 'commit'
    CHECK (mode IN ('dry_run', 'commit')),
  status TEXT NOT NULL DEFAULT 'running'
    CHECK (status IN ('running', 'completed', 'failed', 'rolled_back')),
  total_records INTEGER NOT NULL DEFAULT 0,
  created_count INTEGER NOT NULL DEFAULT 0,
  updated_count INTEGER NOT NULL DEFAULT 0,
  skipped_count INTEGER NOT NULL DEFAULT 0,
  error_count INTEGER NOT NULL DEFAULT 0,
  summary_json TEXT,
  error_log TEXT
);

CREATE TABLE import_run_items (
  id TEXT PRIMARY KEY,
  import_run_id TEXT NOT NULL
    REFERENCES import_runs(id) ON DELETE CASCADE,
  content_item_id TEXT
    REFERENCES content_items(id) ON DELETE SET NULL,
  source_record_key TEXT NOT NULL,
  action TEXT NOT NULL
    CHECK (action IN ('create', 'update', 'skip', 'error')),
  match_strategy TEXT, -- external_id, canonical_url, content_hash, fuzzy_title
  before_snapshot_json TEXT,
  after_snapshot_json TEXT,
  message TEXT
);

CREATE TABLE obsidian_sync_state (
  content_item_id TEXT PRIMARY KEY
    REFERENCES content_items(id) ON DELETE CASCADE,
  note_path TEXT NOT NULL UNIQUE,
  note_template TEXT,
  last_pushed_at TEXT,
  last_export_hash TEXT,
  last_pull_reviewed_at TEXT,
  sync_mode TEXT NOT NULL DEFAULT 'push'
    CHECK (sync_mode IN ('push', 'ignore'))
);
```

### 8.4 — FTS5 Virtual Table and Critical Indexes

**Recommendation**: use a **denormalized contentless FTS5 table** maintained by the application layer. That avoids fragile multi-table trigger logic and works well with tags and annotations.

```sql
CREATE VIRTUAL TABLE content_fts USING fts5(
  content_item_id UNINDEXED,
  title,
  excerpt,
  body_text,
  annotation_text,
  author,
  source_title,
  tags,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE INDEX idx_content_items_type_status
  ON content_items(content_type, status);

CREATE INDEX idx_content_items_starred
  ON content_items(is_starred, archived_at);

CREATE INDEX idx_content_items_canonical_url
  ON content_items(canonical_url);

CREATE INDEX idx_feed_subscriptions_folder
  ON feed_subscriptions(feed_folder_id, sort_title);

CREATE INDEX idx_feed_fetch_runs_feed_started
  ON feed_fetch_runs(feed_subscription_id, started_at);

CREATE INDEX idx_external_refs_content
  ON external_refs(content_item_id, provider);

CREATE INDEX idx_annotations_content_created
  ON annotations(content_item_id, created_at);

CREATE INDEX idx_review_state_due
  ON review_state(scheduled_for, suspended);

CREATE INDEX idx_content_tags_tag
  ON content_tags(tag_id, content_item_id);

CREATE INDEX idx_collection_items_collection
  ON collection_items(collection_id, position);

CREATE INDEX idx_import_run_items_run
  ON import_run_items(import_run_id, action);
```

**Search indexing rule**: rebuild or patch `content_fts` whenever any of these change:

- `content_items.title`
- `content_items.excerpt`
- `content_bodies.body_text`
- annotation quote/note text
- tag membership
- source title/author

### 8.5 — Migration Strategy

**Recommendation**: versioned SQL migrations in the storage crate, applied transactionally on startup, with `PRAGMA user_version` plus a migration history table.

**Rules**:

1. **Never mutate migrations in place.**
2. Prefer **additive migrations** first; backfill later.
3. Large derived structures like FTS can be rebuilt after schema migration rather than migrated row-by-row.
4. Keep destructive cleanup as explicit follow-up migrations.
5. Any migration that rewrites large tables must be benchmarked against realistic libraries.

**Suggested migration layout**:

- `V001__core_schema.sql`
- `V002__fts_and_indexes.sql`
- `V003__obsidian_sync_state.sql`
- `V004__newsletter_support.sql`
- `V005__fsrs_review_state.sql`

**Operational strategy**:

- Run migrations inside a transaction where possible.
- Put the DB in **WAL mode**.
- Rebuild FTS after relevant migrations rather than trying to preserve it.
- Export a backup before any major-version schema upgrade.
- Keep importer/output code tolerant of at least one previous export schema version.

---

## Section 9: Import & Export

Import/export is not a supporting feature for pergamon. It is one of the main reasons the product should exist at all. The user is replacing several existing tools; migration quality is part of the product promise.

### 9.1 — Import Source Matrix

| Source | Format | What Comes In | Phase | Complexity | Validation |
|---|---|---|---|---|---|
| Direct RSS/Atom feed | XML URL | Feed metadata + entries | MVP | 🟢 | [Validated] |
| Website feed autodiscovery | HTML + feed link tags | Site URL → feed URL(s) | MVP | 🟢 | [Validated] |
| OPML | `.opml` | Feed subscriptions + folder/group info | MVP | 🟢 | [Validated] |
| Inoreader subscriptions | OPML export | Feed list + folder structure | MVP | 🟢 | [Validated] |
| Inoreader saved/starred/history export | Service export/API | Read state, saved items, stars, tags | Phase 2 | 🟡 | [Validation Required] |
| Netscape bookmark HTML | `.html` | Browser bookmarks/folders | MVP | 🟢 | [Validated] |
| Raindrop.io export | CSV/JSON | URLs, tags, collections, notes, dates | Phase 2 | 🟡 | [Validation Required] |
| Readwise export/API | CSV/API | Highlights, notes, source refs, tags, review context | Phase 2 | 🟢 | [Implemented] |
| Readwise Reader export/API | CSV/JSON/API | Saved articles, highlights, read state, tags | Phase 2 | 🔴 | [Validation Required] |
| Kindle `My Clippings.txt` | Plain text | Highlights, notes, location markers | MVP | 🟢 | [Validated] |
| Local PDF import | File/folder | PDF asset, metadata, extracted text, hash | MVP | 🟢 | [Validated] |
| `.eml` / Maildir newsletters | RFC 822 email files | Newsletter issue content + metadata | Phase 5 | 🟡 | [Validated] |
| IMAP newsletter folder | IMAP | Live newsletter ingestion | Phase 6 | 🔴 | [Validation Required] |
| Generic CSV/JSON | User-mapped fields | Flexible bookmark/article/highlight imports | Phase 3 | 🟡 | [Validated] |
| Obsidian note pull-back | Markdown + YAML | Manual note import for matched `folio_id`s | Phase 4 | 🟡 | [Validated] |

**Launch priority**:

1. RSS/Atom + OPML
2. browser bookmarks HTML
3. PDF import
4. Kindle clippings
5. Raindrop / Readwise / Reader migrations

That ordering gets pergamon usable fast while still targeting the services it replaces.

**Important import truth**: Readwise and Reader are strategically important, but their exact export/API surfaces should be treated as **validation gates**, not assumptions.

### 9.2 — Import Architecture

**Recommendation**: every importer should implement the same pipeline:

1. **Parse** source file/API into raw records.
2. **Normalize** into a common `ImportCandidate`.
3. **Match** against existing content using deterministic rules.
4. **Preview** a dry-run diff.
5. **Commit** changes transactionally.
6. **Journal** the run for undo/audit.

#### Dry-Run

Dry-run is mandatory.

Example behavior:

- counts new vs matched vs conflicted records,
- shows top-level mapping summary,
- samples a few likely duplicates,
- never writes to operational tables.

For migration-heavy products, dry-run builds trust before the first destructive mistake.

#### Idempotency

**Recommendation**: idempotency should be driven by a match cascade:

1. `provider + external_id`
2. canonical URL
3. content hash / asset hash
4. exact title + author/source match
5. fuzzy match only with user confirmation

This should be recorded in `import_run_items.match_strategy`.

**Rule**: re-importing the same export should not create duplicates.

#### Provenance

Provenance should exist at two levels:

- **record-level** via `external_refs`
- **field-level** via `content_field_provenance`

That allows pergamon to answer:

- where an item came from,
- which field was set by which importer,
- whether the user has since overridden it.

**User-edit precedence rule**:

- once the user edits a field locally, that field should be marked as a user override,
- later imports may suggest changes, but should not silently overwrite user-owned fields.

#### Rollback

Rollback should be explicit and auditable.

**Recommendation**:

- smaller imports: single transaction, simple rollback
- larger imports: chunked commit + inverse journal in `import_run_items`

That enables:

- `pergamon import undo <run-id>`
- removal of created items,
- restoration of updated items from `before_snapshot_json`,
- clear user-visible summaries after rollback

#### Import UX

Every importer should produce the same four user-facing outputs:

- parse summary,
- match summary,
- warning list,
- final result counts

That consistency matters more than fancy per-importer UI.

### 9.3 — Export System

Exports should be both **human-usable** and **lossless**.

| Format | Purpose | Phase | Complexity | Validation |
|---|---|---|---|---|
| OPML | Feed portability | MVP | 🟢 | [Validated] |
| Netscape bookmarks HTML | Browser / bookmark app portability | MVP | 🟢 | [Validated] |
| JSON (queryable) | Programmatic export, automation, debugging | MVP | 🟢 | [Validated] |
| Markdown note bundle | Obsidian and plain-text workflows | Phase 2 | 🟢 | [Validated] |
| CSV | Lightweight spreadsheet/report export | Phase 2 | 🟢 | [Validated] |
| ZIP backup (canonical) | Full-fidelity backup and migration | Phase 2 | 🟡 | [Validated] |
| Highlight package | Quotes/annotations only, filtered by query | Phase 3 | 🟢 | [Validated] |
| Static HTML export | Publish/share personal archive locally | Phase 5 | 🟡 | [Validation Required] |

**Export rules**:

- every export should accept a **query filter**
- every export should include enough metadata to round-trip where appropriate
- export should never depend on a hosted service
- raw binary assets should be optional except in canonical backup mode

**Recommended commands**:

- `pergamon export opml`
- `pergamon export bookmarks-html`
- `pergamon export json --query 'tag:ai archived:true'`
- `pergamon export markdown --collection Research`
- `pergamon export backup`

### 9.4 — Canonical Lossless Export Format

**Recommendation**: a single **ZIP archive containing JSON documents plus content assets**.

```text
pergamon-export-2026-03-18.zip
├── manifest.json
├── schema-version.txt
├── content_items.json
├── content_bodies.json
├── feeds.json
├── annotations.json
├── review_cards.json
├── review_logs.json
├── tags.json
├── collections.json
├── saved_searches.json
├── external_refs.json
├── import_runs.json
├── obsidian_sync_state.json
└── assets/
    ├── pdf/
    │   └── <sha256>.pdf
    ├── snapshots/
    │   └── <sha256>.html
    └── raw/
        └── <sha256>.eml
```

**Why this format is right**:

- one portable file,
- inspectable with standard tools,
- friendly to scripts,
- stable enough for long-term backup,
- not tied to SQLite internals,
- includes real assets rather than only metadata.

**What must be included**:

- all canonical entity tables,
- annotations and review history,
- tags/collections/search definitions,
- external references and provenance,
- binary assets needed for restoration.

**What should be excluded**:

- FTS indexes,
- feed fetch caches,
- transient HTTP metadata,
- rebuildable derived indexes.

**Round-trip promise**:

`pergamon export backup` → `pergamon import backup` should restore:

- the same content items,
- the same tags/collections,
- the same review card states,
- the same external provenance,
- the same attached assets,
- and materially the same Obsidian mapping state.

That round-trip guarantee is the real moat for a personal data product.

**Manifest recommendation**:
`manifest.json` should include:

- export timestamp,
- pergamon version,
- schema version,
- item counts by content type,
- asset counts and checksums,
- generator platform,
- optional warnings.

A short example:

```json
{
  "format": "pergamon-backup",
  "schema_version": 1,
  "generated_at": "2026-03-18T12:00:00Z",
  "folio_version": "0.4.0",
  "counts": {
    "content_items": 18234,
    "annotations": 9412,
    "review_cards": 1201,
    "assets": 684
  }
}
```

**Final export decision**: pergamon should optimize for **open restoration**, not just “download your data.” A backup the user cannot understand or re-import is not good enough for a sovereignty-first product.

---

## Section 10: Competitive Analysis

> **Legend:** 🟢 low execution risk, 🟡 moderate, 🔴 high, ⛔ deliberately deferred.  
> **Confidence tags:** **[Validated]** = strong enough to commit into the roadmap now. **[Validation Required]** = needs a spike, legal check, or third-party verification before locking in.

### 10.1 RSS Readers

| Product | What it does well | Where it breaks for pergamon’s target user | Strategic takeaway |
|---|---|---|---|
| **Inoreader** | Best-in-class filters, rules, newsletter ingestion, mature mobile/web UX | Cloud-first, expensive at higher tiers, weak ownership story, no unified highlights/bookmarks/retention loop | pergamon should not chase enterprise feed ops; it should win on ownership, portability, and integration depth. **[Validated]** |
| **Feedly** | Polished UX, good source discovery, strong team/news use cases | SaaS-first, weak local-first story, not a personal knowledge system | Feedly is a workflow tool; pergamon should be a personal archive. **[Validated]** |
| **Miniflux** | Excellent self-hosted RSS reader, focused scope, fast, sane defaults | Narrow scope: not a bookmark manager, not a highlight/review engine, limited capture ecosystem | Miniflux proves focused RSS can be durable; pergamon should borrow its restraint, not its narrowness. **[Validated]** |
| **FreshRSS** | Flexible self-hosted RSS, many extensions, free | Admin-heavy, inconsistent UX, weak “collected knowledge” layer | Self-hosting alone is not the product. pergamon must feel cohesive out of the box. **[Validated]** |
| **Newsboat** | Powerful terminal RSS, scriptable, beloved by power users | Text-only, manual setup, no reader-mode pipeline, no bookmark/highlight unification | Newsboat validates CLI-first demand. pergamon can inherit that audience with a richer library model. **[Validated]** |
| **NetNewsWire** | Clean Apple-native RSS, offline-friendly, elegant reading | Apple-only, no unified bookmarks/highlights, limited knowledge export | Native polish matters, but pergamon must stay cross-platform and library-centric. **[Validated]** |

**Decision:** pergamon should position against **Inoreader’s breadth** and **Newsboat’s ergonomics** at the same time. That means: strong feeds, local ownership, keyboard-first workflows, and a canonical item model that can later absorb bookmarks, highlights, and review without becoming a bolt-on mess. **[Validated]**

### 10.2 Read Later

| Product | What it does well | Where it breaks for pergamon’s target user | Strategic takeaway |
|---|---|---|---|
| **Readwise Reader** | Strong ingestion breadth, email-to-reader, highlights, read-later + RSS blend | Closed SaaS, weak local ownership, subscription-first, tied to Readwise ecosystem | This is the closest modern benchmark. pergamon should match the integrated workflow, then beat it on ownership and openness. **[Validated]** |
| **Pocket** | Frictionless capture, mainstream brand, simple read-later mental model | Lightweight organization, weak export story, limited knowledge retention, cloud dependency | “Save it for later” is not enough anymore. pergamon must turn capture into a durable library. **[Validated]** |
| **Instapaper** | Mature reading UX, dependable text extraction, annotations | Narrow scope, aging product energy, weak graph to bookmarks/RSS/knowledge reuse | Good reading UX is table stakes, not a moat. **[Validated]** |
| **Omnivore** *(shut down)* | Was the clearest “reader + highlights + API + open-source leaning” option | Market fragility; users learned that hosted convenience can disappear fast | Omnivore’s shutdown is pergamon’s clearest market opening. Reliability and exportability are not side features; they are the product. **[Validated]** |
| **Wallabag** | Self-hosted, privacy-friendly, article saving works | Rougher UX, fragmented polish, weaker knowledge loop | pergamon should learn from Wallabag’s ownership story but avoid self-host-first UX debt. **[Validated]** |
| **Hoarder** | Modern self-hosted saving, media support, promising organization model | More “save everything” than “read deeply,” weaker review/annotation story | Capture alone is not differentiation. Retrieval and resurfacing must be first-class. **[Validated]** |

**Decision:** pergamon should define its read-later layer as **“owned reader mode plus downstream reuse”**, not just saving URLs. The win condition is not “saved page fidelity”; it is “this page becomes part of my long-term personal corpus.” **[Validated]**

### 10.3 Bookmark Managers

| Product | What it does well | Where it breaks for pergamon’s target user | Strategic takeaway |
|---|---|---|---|
| **Raindrop.io** | Best bookmark UX, polished collections, media previews, cross-platform | SaaS-first, limited deep reading/retention flow, weaker local-first story | Raindrop is the UX bar for bookmark organization. pergamon should match the information architecture, not the visual curation emphasis. **[Validated]** |
| **Pinboard** | Durable, simple, fast, power-user credibility | Sparse UX, aging product, no reader/highlight/review integration | pergamon can inherit Pinboard’s trust while feeling contemporary. **[Validated]** |
| **LinkAce** | Self-hosted bookmarks with decent tagging and lists | Admin burden, weaker capture ergonomics, smaller ecosystem | Self-hosting is attractive to power users, but defaults must be better than a hobby admin panel. **[Validated]** |
| **Shiori** | Lightweight self-hosted archival/bookmark blend | Narrower organization model, less polish, small ecosystem | Good inspiration for local archive semantics, not enough for the whole product. **[Validated]** |
| **Linkding** | Fast, clean, self-hosted bookmark manager | Focused on storage, not reading or long-term learning | Useful benchmark for “simple and reliable,” but pergamon must go beyond CRUD. **[Validated]** |

**Decision:** pergamon should replace Raindrop and Pinboard by emphasizing **capture quality + canonical URL identity + search + export + reuse**, not public collections, social discovery, or heavy visual dashboards. **[Validated]**

### 10.4 Highlight & Knowledge Management

| Product | What it does well | Where it breaks for pergamon’s target user | Strategic takeaway |
|---|---|---|---|
| **Readwise** | Best resurfacing loop, spaced repetition adjacent, broad import ecosystem | Closed cloud workflow, expensive, highlights not fully user-owned in practice | pergamon should copy the retention loop, not the lock-in. **[Validated]** |
| **Matter** *(sunset)* | Beautiful reading and highlighting experience | Product instability proved polish without durability is not enough | Sunset risk is now a competitive talking point: local-first products age better than VC experiments. **[Validated]** |
| **Glasp** | Social highlighting and web annotation | Social-first, public-by-default feel, not a private archive | pergamon should explicitly reject social mechanics. Private curation is the point. **[Validated]** |
| **Hypothesis** | Excellent annotation on the open web, strong academic use | Web annotation first, personal library second, not an integrated ingestion system | Annotation alone is insufficient. pergamon needs source ingestion, storage, resurfacing, and export in one loop. **[Validated]** |

**Decision:** pergamon’s highlight system should be **source-agnostic, exportable, and reviewable**. Highlights are not vanity artifacts; they are inputs to future retrieval, spaced repetition, and note workflows. **[Validated]**

### 10.5 Unified / Adjacent Competitors

| Product | What it gets right | Why it still leaves room for pergamon | Strategic takeaway |
|---|---|---|---|
| **Omnivore** *(was)* | Unified RSS/read-later/highlights came closest to the target shape | Shutdown destroyed trust; self-host story never fully became the center of the product | pergamon should inherit the problem statement, then solve the trust problem. **[Validated]** |
| **Logseq** | Local-first knowledge graph, block-based capture, daily notes workflow | Ingestion is fragmented; readers still need separate RSS/save/highlight systems | Logseq is downstream thinking, not upstream ingestion. **[Validated]** |
| **Obsidian + plugins** | Flexible, huge plugin ecosystem, strong local ownership, strong notes UX | Plugin stacks are brittle, ingestion is fragmented, too much user assembly required | pergamon should be the ingestion engine; Obsidian should stay the note workspace. **[Validated]** |

**Decision:** pergamon should not try to become a full PKM editor. It should become the **canonical ingestion and resurfacing layer** that feeds tools like Obsidian cleanly. That is a sharper and more defensible product boundary. **[Validated]**

### 10.6 Differentiation Analysis

#### 2x2 Matrix: Data Ownership vs Workflow Unification

```text
                           High data ownership / local-first
                                         ↑
                    Newsboat •           │        pergamon ●
                    Miniflux •           │        Obsidian+plugins ◐
                    NetNewsWire •        │
                    Linkding •           │
 Narrow / single-purpose ────────────────┼────────────────────────→ Unified workflow
                    Pocket •             │        Omnivore (was) ◐
                    Feedly •             │        Readwise Reader ◐
                    Raindrop •           │
                    Inoreader •          │
                                         ↓
                           Low ownership / cloud dependence
```

#### pergamon’s differentiation thesis

1. **Unified without becoming bloated.**  
   pergamon unifies **feeds, read-later, bookmarks, highlights, and review** around one item model instead of five loosely connected products. **[Validated]**

2. **Local-first by default, not as a premium mode.**  
   Nearly every incumbent treats local ownership as secondary, optional, or nonexistent. pergamon makes owned storage the default and hosted sync an optional convenience. **[Validated]**

3. **CLI/TUI-first as a feature, not a constraint.**  
   No serious competitor combines modern terminal ergonomics with long-form reading, capture, organization, and resurfacing. That gives pergamon a real wedge with power users. **[Validated]**

4. **Obsidian-native downstream integration instead of “all notes live here.”**  
   Obsidian wins the note-editing layer. pergamon should win the ingestion layer and export excellent materials into the user’s vault. **[Validated]**

#### Why now

- **Users are tired of paying for four separate tools.** Inoreader + Readwise + Reader + Raindrop is a real cost stack with overlapping mental models. **[Validated]**
- **VC-backed fragility changed buyer psychology.** Omnivore’s shutdown and Matter’s sunset shifted “can I export?” from edge concern to purchase criterion. **[Validated]**
- **The Rust/SQLite/WASM/UniFFI toolchain is finally good enough.** A solo developer can now credibly ship a cross-platform, local-first stack without abandoning performance or portability. **[Validated]**
- **AI raises the value of owned corpora.** If personal knowledge systems become retrieval and synthesis layers, the upstream archive matters more than ever. pergamon can own that archive without making AI the wedge. **[Validated]**
- **Algorithmic feeds pushed power users back toward deliberate reading.** RSS, bookmarking, and private archives are resurging precisely because public feeds are noisier. **[Validated]**

**Bottom line:** pergamon is not “open-source Pocket” or “self-hosted Readwise.” It is a **portable personal ingestion system** for people who want one owned library instead of four rented silos. **[Validated]**

---

## Section 11: Licensing & Open Source Strategy

### 11.1 License Recommendation

**Recommendation:** use the same split-license pattern as **ldgr/tock** — **Apache-2.0 for everything client-side and shared**, **AGPL-3.0 for the sync server only**. **[Validated]**

| Scope | License | Why |
|---|---|---|
| `pergamon-core`, CLI, TUI, WASM bindings, iOS bindings, Obsidian plugin, docs, tests | **Apache-2.0** | Contributor-friendly, explicit patent grant, commercially compatible, ideal for libraries and client apps. |
| `pergamon-server` (Axum sync server) | **AGPL-3.0** | Prevents a hosted sync fork from becoming a closed SaaS while keeping the protocol open. |
| Protocol docs / export specs | **Apache-2.0** | Encourages ecosystem adapters and importer/exporter tooling. |

**Why this split is right for pergamon:**

- pergamon’s real moat should be **product quality and trust**, not a hostile license on the client. **[Validated]**
- The only part with real “service capture” risk is the **networked sync server**; AGPL is a precise response to that risk. **[Validated]**
- Apache-2.0 keeps the core reusable for side tools, experiments, and integrations, including future importer tooling. **[Validated]**
- This matches kafkade’s existing architecture philosophy and avoids introducing a new legal model just for pergamon. **[Validated]**

**Guardrail:** keep the server boundary clean. Do not move AGPL-only code into shared crates that the CLI, iOS, or web clients need. If a crate is shared, it should remain Apache-2.0. **[Validated]**

### 11.2 Contribution Model

**Recommendation:** lightweight contributor process, no CLA, strong docs, ADR-driven architecture. **[Validated]**

Core components:

- **DCO instead of CLA.**  
  Require `Signed-off-by` on commits. For a solo-maintained open-source project, DCO keeps legal friction low and is easier to explain than a CLA. **[Validated]**

- **`CONTRIBUTING.md`.**  
  Include:
  - workspace layout and crate boundaries
  - zero-I/O rule for `pergamon-core`
  - test/lint commands
  - fixture-driven importer expectations
  - licensing guidance for server vs non-server code  
  **[Validated]**

- **`CODE_OF_CONDUCT.md` + `SECURITY.md`.**  
  Use Contributor Covenant and a clear security disclosure path. Security matters more than average because pergamon will store sensitive reading and annotation history. **[Validated]**

- **ADR workflow.**  
  Any change to sync, item identity, storage schema, Obsidian contract, or cross-platform boundaries should go through an ADR. This is especially important for a solo developer project that wants outside help without architecture drift. **[Validated]**

- **Issue templates and labels.**  
  Use:
  - bug
  - importer bug
  - reader extraction issue
  - docs improvement
  - good first issue
  - needs fixture  
  Importers and extractors benefit heavily from reproducible fixtures. **[Validated]**

**Contribution strategy:** attract contributors on **integration edges**, not the deepest core first. Good first contributions are importers, docs, test fixtures, Obsidian plugin polish, and UI improvements — not sync cryptography. **[Validated]**

### 11.3 Monetization

**Recommendation:** **GitHub Sponsors** as the default support channel, plus **optional managed sync hosting** once the sync protocol is stable. No feature paywall on local-first workflows. **[Validated]**

| Revenue path | Role | Recommendation |
|---|---|---|
| GitHub Sponsors | Base support / patronage | Start here immediately. It aligns with open source and adds no product distortion. |
| Optional managed sync hosting | Convenience revenue | Add only after Phase 7. Charge for reliability and hosting, not access to core features. |
| Premium feature lockouts | Artificial monetization | Reject. It undermines the project’s trust story. |
| Ads / data monetization | Misaligned | Reject entirely. |

**Why this model fits pergamon:**

- The core promise is **ownership**. A hard SaaS paywall would contradict that from day one. **[Validated]**
- Managed sync is a legitimate paid convenience because it saves users time without taking their data hostage. **[Validated]**
- Sponsors create a sustainability layer before the product needs support operations. **[Validated]**
- Self-hosting must remain first-class. If managed sync exists, export and self-host must stay excellent. **[Validated]**

**Commercial stance:** pergamon should be **open-source software with optional hosting**, not an “open core” feature maze. **[Validated]**

---

## Section 12: Tech Stack Recommendations

### 12.0 Recommended stack by component

| Component | Recommendation | Why this is the right call |
|---|---|---|
| **Core domain** | Rust workspace with **zero-I/O `pergamon-core`** | Keeps parsing, identity, retention logic, and conflict rules testable and portable to CLI, web, and iOS. **[Validated]** |
| **CLI** | `clap v4` | Best-in-class argument model, completions, manpage support, and consistency with kafkade project DNA. **[Validated]** |
| **TUI** | `ratatui` + `crossterm` | Mature Rust terminal UI stack; enough control for inbox, reader, review queue, and search flows. **[Validated]** |
| **Long-form text rendering** | `termimad` | Best balance of readable markdown-ish rendering inside a terminal without inventing a custom renderer too early. **[Validation Required]** |
| **Local storage** | SQLite + FTS5 via `rusqlite` | Single-file, portable, excellent full-text search, easy backups, stable across desktop/mobile/web adapter boundaries. **[Validated]** |
| **Migrations** | `refinery` | Simple embedded SQL migrations and version tracking without overengineering. **[Validated]** |
| **HTTP** | `reqwest` + `rustls` | Mature async HTTP, predictable TLS, portable across targets that need networking. **[Validated]** |
| **Feed parsing** | `feed-rs` | Best fit for RSS/Atom/JSON Feed parsing in Rust with a sane normalized model. **[Validated]** |
| **Article extraction** | `readability` crate | Fastest route to a readable “reader mode” without building extraction heuristics from scratch. **[Validation Required]** |
| **HTML sanitization** | `ammonia` | Safe allowlist-based sanitization for extracted content and imported HTML fragments. **[Validated]** |
| **Spaced repetition** | `fsrs` | Best available scheduling model for modern retention workflows; clearly superior to inventing a homegrown SM-2 clone. **[Validated]** |
| **Serialization** | `serde`, `serde_json`, `toml` | Standard Rust ecosystem choices; no reason to diverge. **[Validated]** |
| **IDs** | UUIDv7 or ULID | Stable, sortable identifiers work well for sync, exports, and item provenance. **[Validated]** |
| **Sync server** | `axum` + encrypted blob/event storage | Clean Rust fit, easy middleware, good ecosystem, aligns with existing kafkade server playbook. **[Validated]** |
| **Web client** | WASM-compiled core + thin TypeScript shell | Keep domain logic in Rust, keep web UI iteration speed reasonable, avoid committing to a heavy all-Rust frontend too early. **[Validation Required]** |
| **iOS** | UniFFI-generated bindings + SwiftUI wrapper | Cleanest path from shared Rust logic to native iOS UX without hand-maintaining C FFI everywhere. **[Validated]** |
| **Obsidian plugin** | TypeScript using the official Obsidian API | Obsidian plugins are TypeScript-native; keep the plugin thin and drive integration through files and stable APIs. **[Validated]** |
| **Observability** | `tracing` + structured logs in adapters only | Core stays pure; shells and server get diagnostics. **[Validated]** |
| **Testing** | fixture-driven integration tests + snapshot tests + property tests | Importers, extractors, and sync behaviors all benefit from real fixtures more than unit tests alone. **[Validated]** |

### Key technical decisions

#### Feed parsing: **`feed-rs`**  

This is the correct choice for pergamon because the first problem to solve is *trustworthy normalized ingestion*, not squeezing out exotic parser edge cases via a custom implementation. `feed-rs` gets you to “real feeds work” quickly and keeps the complexity in `pergamon-core` where it belongs. **[Validated]**

#### Reader extraction: **`readability` crate**  

This is the right default because pergamon needs reader mode early to be credible as a Reader/Pocket replacement. The risk is not correctness; it is **quality variance** on ugly pages, newsletters, and JS-heavy sites. The product answer is not “build our own extractor”; it is “use `readability`, then fall back cleanly to original content when extraction is bad.” **[Validation Required]**

#### Spaced repetition: **FSRS**  

If pergamon is going to replace Readwise’s resurfacing loop, it should not settle for a simplistic scheduler. FSRS gives pergamon a modern retention engine and future-proofs review data instead of locking the product into a weak heuristic. **[Validated]**

#### HTML sanitization: **`ammonia`**  

Any ingestion system that stores extracted HTML needs a hard sanitization layer. `ammonia` is the boring, correct choice. This should be treated as mandatory infrastructure, not optional polish. **[Validated]**

#### TUI rendering: **`termimad`**  

The terminal reader must feel like reading, not like scrolling raw text blobs. `termimad` is a good fit for headers, blockquotes, code fences, lists, and links without inventing a bespoke renderer. The key question is performance and readability on long articles; that is a spike, not a reason to avoid the library. **[Validation Required]**

#### HTTP transport: **`reqwest` + `rustls`**  

Predictable, portable, well-maintained, and aligned with the rest of the Rust ecosystem. No need to get clever here. **[Validated]**

### Additional stack recommendations

- **Canonical item model:** one `items` table with typed subtype records or JSON payloads for feed entries, saved pages, bookmarks, notes, and highlights. This minimizes cross-feature duplication. **[Validated]**
- **Search:** FTS5 over title, source, author, extracted text, tags, and notes; do not introduce Tantivy unless SQLite proves insufficient. **[Validated]**
- **Import strategy:** file-based import first (OPML, Netscape bookmarks, Readwise export, My Clippings). Avoid unofficial APIs until a real user need proves them necessary. **[Validated]**
- **Export strategy:** Markdown + JSON + full encrypted backup from day one. This is part of the trust story, not a later feature. **[Validated]**
- **Sync storage:** start with SQLite metadata + filesystem/object storage for opaque encrypted blobs on the server. Do not overdesign multi-tenant infrastructure before Phase 7. **[Validation Required]**

### 12.1 Proof-of-Concept Gates

| Gate | What to prove | Success criteria | Complexity | Status |
|---|---|---|---|---|
| Feed corpus parse | `feed-rs` handles representative RSS/Atom/JSON feeds | 25 real feeds parse with stable IDs, titles, dates, and content fallbacks | 🟢 | **[Validated]** |
| Reader extraction quality | `readability` is good enough to ship Phase 0/1 | 80%+ of a 30-page corpus render acceptably without manual cleanup | 🟡 | **[Validation Required]** |
| Sanitization fidelity | `ammonia` removes unsafe markup without destroying readability | code blocks, lists, links, and images survive in 90%+ of extraction fixtures | 🟢 | **[Validation Required]** |
| Terminal reading UX | `termimad` + `ratatui` are pleasant for long-form reading | 10-minute reading session is comfortable in 100x30 and 140x40 terminals | 🟡 | **[Validation Required]** |
| Search performance | SQLite + FTS5 can carry the corpus | <100 ms query latency on 100k items / 1M highlight rows on a laptop | 🟢 | **[Validated]** |
| FSRS integration | Review queue semantics fit pergamon highlights | create/review/reschedule loop works deterministically across 1k test cards | 🟢 | **[Validated]** |
| WASM footprint | Web build stays reasonable | core WASM bundle <2 MB compressed for first web feature slice | 🔴 | **[Validation Required]** |
| UniFFI bridge | Shared Rust core is viable on iOS | demo SwiftUI app can list/search/open locally stored items via UniFFI | 🟡 | **[Validation Required]** |
| Obsidian export contract | export format is stable enough for a plugin | exported Markdown + JSON index re-import cleanly into a sample vault | 🟡 | **[Validation Required]** |

**Decision:** do not leave Phase 0 until the extraction, rendering, and search gates are green enough to support daily reading. If the reading loop is weak, everything else becomes shelfware. **[Validated]**

---

## Section 13: Phased Roadmap with Milestones

### Phase 0: First Feed 🟢

**Theme:** *“One feed, one article, end to end.”*  
**Goal:** prove the core stack by subscribing to a feed, fetching entries, storing them locally, and opening a readable article in the CLI/TUI. **[Validated]**

**Deliverables**

1. Cargo workspace with `pergamon-core`, `pergamon-cli`, and storage crate boundaries. **[Validated]**
2. Feed subscription + fetch pipeline using `feed-rs`, ETag, and Last-Modified support. **[Validated]**
3. SQLite schema + FTS5 index for items, feeds, read state, and extracted content. **[Validated]**
4. TUI inbox + basic article reader using `ratatui` + `termimad`. **[Validation Required]**
5. Fixture corpus for feeds and article extraction regressions. **[Validated]**

**Acceptance criteria for top 3**

- `pergamon feed add <url>` followed by `pergamon sync` ingests 10 representative feeds with stable item identity.  
- `pergamon list` can display and filter a 1,000-item corpus with no visible lag.  
- `pergamon open <item>` renders readable extracted content on a representative test corpus with acceptable fidelity.  

**Dependencies**

- None beyond repo bootstrap and the PoC gates in Section 12.1.

**Risks**

- Extracted content quality varies more than expected. **Mitigation:** clean fallback to original link view. **[Validation Required]**
- Feed identity edge cases create duplicates. **Mitigation:** retain source IDs and provenance metadata. **[Validated]**

**Cut line**

- No OPML import.
- No bookmark capture.
- No highlights or review queue.
- No web or iOS.

**ADRs to write**

- ADR-001 Workspace and crate boundaries.
- ADR-002 Canonical item model and storage schema.
- ADR-003 Feed identity, polling, and dedupe rules.

---

### Phase 1: Minimum Usable Reader (MVP) 🟡

**Theme:** *“I can replace my daily reading stack.”*  
**Goal:** ship a credible local-first replacement for core RSS reading plus basic read-later and bookmark capture. This is the first phase where pergamon must become part of the maintainer’s daily routine. **[Validated]**

**Deliverables**

1. OPML import, feed folders, source muting, and bulk feed management. **[Validated]**
2. Reader TUI with unread/star/archive/save-later flows and keyboard-first triage. **[Validated]**
3. `pergamon save <url>` for manual bookmark/read-later capture with extraction and dedupe. **[Validated]**
4. Full-text search across title, source, tags, and extracted text. **[Validated]**
5. Encrypted/local backup export and restore flow. **[Validated]**

**Acceptance criteria for top 3**

- Importing a 100-feed OPML file preserves titles and URLs with >95% fidelity and completes in one obvious flow.  
- A user can process 500 unread items in the TUI without needing the mouse or leaving the keyboard.  
- `pergamon save https://example.com/article` dedupes canonical duplicates and stores title, domain, excerpt, and extracted text.  

**Dependencies**

- Phase 0 complete.
- Extraction, search, and rendering gates green enough for dogfooding.

**Risks**

- Too much UI ambition slows shipping. **Mitigation:** prefer fast list/detail flows over ornamental polish. **[Validated]**
- Bookmark identity is trickier than feed identity. **Mitigation:** keep raw URL, canonical URL, and source provenance separately. **[Validated]**

**Cut line**

- No browser extension.
- No smart collections.
- No highlight review queue.
- No sync.

**ADRs to write**

- ADR-004 OPML import semantics and source folders.
- ADR-005 Bookmark canonicalization and duplicate policy.
- ADR-006 Search index scope and ranking.

---

### Phase 2: Bookmark Manager (Raindrop Replacement) 🟡

**Theme:** *“Everything I save lands in one organized library.”*  
**Goal:** turn pergamon from “reader with save support” into a real bookmark manager that can absorb a Raindrop or Pinboard export and stay pleasant to organize over time. **[Validated]**

**Deliverables**

1. Collections, tags, archive semantics, and bulk refile actions for saved links. **[Validated]**
2. Import/export for Netscape bookmarks, Raindrop export, and pergamon backup. **[Validation Required]**
3. Metadata enrichment: Open Graph fields, content type, author, favicon, and domain normalization. **[Validated]**
4. Duplicate detection and merge suggestions across imported libraries. **[Validation Required]**
5. Link health checks and “dead link” detection as a maintenance tool. **[Validation Required]**

**Acceptance criteria for top 3**

- A 5,000-bookmark import preserves URLs, titles, tags, and timestamps at >98% fidelity for supported fields.  
- Collections and tags support fast bulk organization in both CLI and TUI flows.  
- Imported duplicates are caught for exact and canonical URL matches without destructive false merges.  

**Dependencies**

- Phase 1 complete.
- Stable bookmark identity model.

**Risks**

- Import formats are messy and under-documented. **Mitigation:** fixture-driven parsers, provenance fields, and dry-run preview. **[Validated]**
- Dead-link checking can become noisy. **Mitigation:** treat as maintenance metadata, not user-facing panic. **[Validated]**

**Cut line**

- No screenshot archive.
- No public collections.
- No browser extension capture.
- No AI tagging.

**ADRs to write**

- ADR-007 Bookmark import provenance.
- ADR-008 Collection and tag model.
- ADR-009 Duplicate detection and merge safety.

---

### Phase 3: Knowledge Retention (Highlights, FSRS, Kindle, Readwise Import) ✅

**Theme:** *“Saved things become remembered things.”*  
**Goal:** add the retention loop that turns pergamon from archive to personal learning system: highlights, notes, resurfacing, and migration paths from Readwise/Kindle workflows. **[Validated]**

**Deliverables**

1. Unified highlight and note model across reader selections, imported clippings, and imported exports. **[Implemented]**
2. Review queue and spaced repetition scheduling using FSRS. **[Implemented]**
3. Kindle **My Clippings** import and Readwise export import. **[Implemented]**
4. Inline highlighting inside pergamon's reader flows. **[Implemented]**
5. Retention stats: due count, review completion, resurfaced highlights, source breakdown. **[Implemented]**

**Acceptance criteria for top 3**

- Importing 10,000 highlights preserves source, author/title context, original text, and timestamps for supported formats.  
- `pergamon review` produces deterministic due counts and FSRS rescheduling for the same test history on every platform.  
- Highlights created inside pergamon appear in the review queue and export cleanly to Markdown/JSON.  

**Dependencies**

- Phases 1–2 complete.
- Stable item IDs and export model.

**Risks**

- Kindle and Readwise data are inconsistent by source. **Mitigation:** support file-based imports first and preserve raw import payloads. **[Validated]**
- Review UX may feel bolted on. **Mitigation:** keep the loop simple: due → review → resurface/export. **[Validated]**

**Cut line**

- No OCR.
- No AI-generated flashcards.
- No quiz authoring tools.
- No handwriting/PDF markup.

**ADRs to write**

- ADR-010 Highlight and annotation schema.
- ADR-011 FSRS parameters and review semantics.
- ADR-012 Import contracts for Kindle and Readwise migrations.

---

### Phase 4: Obsidian & Polish 🟡

**Theme:** *“pergamon becomes my ingestion engine.”*  
**Goal:** connect pergamon cleanly to Obsidian, add rule-based organization, and polish the product enough that it feels like a stable daily system instead of a powerful prototype. **[Validated]**

**Deliverables**

1. Official Obsidian plugin for browsing pergamon items, inserting references, and importing highlights into notes. **[Validation Required]**
2. Stable Markdown/JSON export contracts with frontmatter, slugs, backlinks, and provenance. **[Validated]**
3. Smart collections and saved searches driven by a query/rule DSL. **[Validated]**
4. Content rules for auto-tagging, auto-archiving, muting, and source-based triage. **[Validated]**
5. Usage stats: inbox size, reading streak, save rate, highlight rate, review completion. **[Validated]**

**Acceptance criteria for top 3**

- An Obsidian user can insert a pergamon item or highlight into a note with stable metadata and source links.  
- Smart collections recompute deterministically and match the same results in CLI, TUI, and exports.  
- Content rules can auto-tag/archive new items based on source, domain, title, or feed without manual intervention.  

**Dependencies**

- Phase 3 complete.
- Stable export shape and item identity.
- Obsidian plugin API validation.

**Risks**

- Plugin scope creep turns pergamon into a note editor. **Mitigation:** keep the plugin thin and file-oriented. **[Validated]**
- Bidirectional sync temptation creates conflict hell. **Mitigation:** start with export/insert flows, not shared editing. **[Validated]**

**Cut line**

- No live bidirectional editing with Obsidian.
- No markdown vault-as-database source of truth.
- No collaborative vault features.

**ADRs to write**

- ADR-013 Obsidian plugin contract.
- ADR-014 Export schema and frontmatter conventions.
- ADR-015 Rule engine/query DSL.

---

### Phase 5: Web Interface (Axum + Web UI + Docker) 🔴

**Theme:** *“The library is available anywhere with a browser.”*  
**Goal:** ship a deployable web interface for browsing, reading, searching, and organizing the pergamon library without abandoning the Rust core or the local-first philosophy. **[Validation Required]**

**Deliverables**

1. Axum-served web application with authentication/session handling and Docker packaging. **[Validated]**
2. Web inbox, reader, bookmarks, highlights, search, and review views powered by the same core logic via WASM or shared validation. **[Validation Required]**
3. Docker image / compose setup for self-hosted deployment. **[Validated]**
4. Progressive enhancement and basic offline caching where practical. **[Validation Required]**
5. Admin diagnostics for feed status, extraction failures, and import logs. **[Validated]**

**Acceptance criteria for top 3**

- A self-hosted user can run pergamon web with Docker in under 10 minutes from documented setup.  
- Search and navigation remain responsive on a 50k-item library in normal desktop browsers.  
- Items created or modified on the web obey the same schema, dedupe rules, and export shape as CLI/TUI flows.  

**Dependencies**

- Phase 4 complete.
- WASM footprint and shared-core boundary validated.
- Basic auth/session model defined.

**Risks**

- Web build complexity explodes. **Mitigation:** keep the client thin and reuse Rust logic aggressively. **[Validation Required]**
- Browser storage/offline semantics tempt an early sync rewrite. **Mitigation:** keep Phase 5 scoped to a deployable web app, not full multi-device sync. **[Validated]**

**Cut line**

- No multi-user features.
- No real-time collaboration.
- No browser extension dependency.
- No shared/public libraries.

**ADRs to write**

- ADR-016 Web architecture and WASM boundary.
- ADR-017 Auth/session model for the web app.
- ADR-018 Docker deployment and server persistence model.

---

### Phase 6: iOS App (SwiftUI via UniFFI) 🔴

**Theme:** *“Capture and review on the phone.”*  
**Goal:** deliver a native-feeling iPhone app for reading, saving, reviewing, and quick ingestion while reusing the Rust core through UniFFI. **[Validation Required]**

**Deliverables**

1. UniFFI bindings and idiomatic Swift wrapper layer. **[Validated]**
2. SwiftUI iPhone app with inbox, saved items, reader, highlights, and review queue. **[Validation Required]**
3. Share extension for saving URLs, selected text, and simple metadata from Safari and supported apps. **[Validation Required]**
4. Local-first offline database and import/export support. **[Validated]**
5. iPad polish only if Phase 6 lands comfortably. **[Validation Required]**

**Acceptance criteria for top 3**

- Shared Rust logic compiles into an XCFramework and is consumed from Swift without manual unsafe glue.  
- A user can save a page from the iOS share sheet into pergamon in under five seconds.  
- Reader mode, search, and review queue work offline after initial sync/import.  

**Dependencies**

- Phase 4 complete.
- UniFFI PoC green.
- Mobile storage ownership clarified.

**Risks**

- UniFFI ergonomics or binary size become painful. **Mitigation:** keep the exposed surface area narrow and wrapper-friendly. **[Validated]**
- Share extension constraints force async compromise. **Mitigation:** use a staging inbox and finalize ingestion in-app when needed. **[Validated]**

**Cut line**

- No widgets.
- No watchOS.
- No advanced iPad layouts.
- No background-heavy sync guarantees before Phase 7.

**ADRs to write**

- ADR-019 UniFFI boundary and error mapping.
- ADR-020 Mobile storage ownership and cache policy.
- ADR-021 Share extension ingestion contract.

---

### Phase 7: Sync & Multi-Device 🔴

**Theme:** *“One library, many devices.”*  
**Goal:** add optional, encrypted multi-device sync through an Axum server without breaking the local-first trust model. This is the hardest phase and should only begin after the single-device experience is already strong. **[Validation Required]**

**Deliverables**

1. AGPL-3.0 Axum sync server storing encrypted blobs/events only. **[Validated]**
2. Device onboarding, key management, and account bootstrap flows. **[Validation Required]**
3. Conflict detection and typed merge policies for read state, tags, notes, and highlights. **[Validation Required]**
4. Managed-hosting-ready deployment plus first-class self-hosting. **[Validated]**
5. Background sync behaviors for web/iOS where feasible. **[Validation Required]**

**Acceptance criteria for top 3**

- Saving or updating an item on one device appears on another device after sync, with the server unable to read plaintext content.  
- Simultaneous edits to the same object either merge safely by type or surface a clear user-visible conflict.  
- New-device bootstrap restores a large library and review state without corrupting identity, rules, or due counts.  

**Dependencies**

- Phase 5 and Phase 6 complete.
- Stable item IDs, exports, and rule engine.
- Server/license boundary already documented.

**Risks**

- Sync complexity consumes the roadmap. **Mitigation:** typed merge policies, narrow scope, and no social/shared libraries. **[Validated]**
- Operational burden overwhelms a solo maintainer. **Mitigation:** self-host first, managed sync only after protocol confidence. **[Validated]**

**Cut line**

- No team accounts.
- No shared family/workspaces.
- No social feeds or following.
- No public link publishing as a sync feature.

**ADRs to write**

- ADR-022 Sync protocol and envelope model.
- ADR-023 Conflict policy by entity type.
- ADR-024 Device onboarding and key lifecycle.

---

### Phase 8: Moonshots (AI, Email Newsletter, Browser Extension, Cross-App) ⛔

**Theme:** *“Make the corpus work for me.”*  
**Goal:** only after the core system is stable, add high-upside convenience features that compound the value of the archive without changing pergamon’s product identity. **[Validation Required]**

**Deliverables**

1. Browser extension for one-click capture, selection capture, and tag-to-inbox workflows. **[Validation Required]**
2. AI-assisted organization/retrieval built with on-device or BYO-model assumptions first. **[Validation Required]**
3. Email newsletter or digest that resurfaces unread backlog, recent saves, and due highlights. **[Validation Required]**
4. Cross-app bridges for richer migration/export loops with Wallabag, Linkding, Readwise, and Obsidian. **[Validated]**
5. Optional semantic clustering, topic views, or “resurface this theme” workflows. **[Validation Required]**

**Acceptance criteria for top 3**

- The browser extension can capture page URL, title, selection, and tags into pergamon in one action.  
- AI features are disabled by default or strictly privacy-bounded; no plaintext leaves the user’s device without explicit opt-in.  
- Weekly digests are useful enough to keep users in the product loop without feeling like a growth hack.  

**Dependencies**

- Phase 7 complete.
- Stable APIs for capture, export, and sync.
- Strong product discipline.

**Risks**

- AI becomes a distraction from the trust story. **Mitigation:** keep AI assistive, optional, and privacy-bounded. **[Validated]**
- Browser extension multiplies support burden. **Mitigation:** start narrow: capture and selection only. **[Validated]**

**Cut line**

- No server-side embeddings by default.
- No social recommendations.
- No algorithmic feed ranking.
- No feature that weakens local-first ownership.

**ADRs to write**

- ADR-025 Browser extension architecture.
- ADR-026 AI privacy and provider boundary.
- ADR-027 Digest generation and opt-in policy.

---

## Section 14: First 90 Days — Execution Plan

### 14.1 Technical Spikes (Weeks 1–2)

| # | Spike | What it proves | Success criteria | Complexity | Status |
|---|---|---|---|---|---|
| 1 | Feed corpus parse | `feed-rs` is viable for real-world ingestion | 25 representative feeds parse and normalize cleanly | 🟢 | **[Validated]** |
| 2 | Article extraction quality | `readability` is good enough for MVP reading | 80%+ acceptable output on a 30-page corpus | 🟡 | **[Validation Required]** |
| 3 | Terminal reader UX | `ratatui` + `termimad` can support long sessions | 10-minute read feels comfortable; no layout glitches on common terminal sizes | 🟡 | **[Validation Required]** |
| 4 | SQLite + FTS5 scale | search won’t be the bottleneck | <100 ms search on 100k mixed items | 🟢 | **[Validated]** |
| 5 | URL canonicalization | bookmark dedupe is safe enough to automate | query params/domain normalization catch obvious dupes without false merges | 🟡 | **[Validation Required]** |
| 6 | WASM build size | web phase remains plausible | first web-oriented bundle stays <2 MB compressed | 🔴 | **[Validation Required]** |
| 7 | UniFFI sample app | iOS path is technically open | sample SwiftUI view lists and opens items from Rust bindings | 🟡 | **[Validation Required]** |
| 8 | Obsidian export loop | plugin scope can stay thin | sample export inserts cleanly into a test vault with stable links | 🟡 | **[Validation Required]** |

**Rule:** do not start broad UI work until spikes 1–4 are green. pergamon’s first job is trustworthy ingestion and readable output. **[Validated]**

### 14.2 First 10 Epics

| # | Epic | Acceptance criteria | Effort | Dependencies | Complexity |
|---|---|---|---|---|---|
| 1 | Project scaffold and CI | Workspace builds on Linux/macOS/Windows; fmt/clippy/test wired; docs skeleton exists | S | None | 🟢 |
| 2 | Canonical item schema + migrations | items, feeds, tags, read state, extraction tables migrate cleanly and round-trip | M | Epic 1 | 🟢 |
| 3 | Feed subscription + sync engine | add/update/sync feeds with ETag support and durable identity | M | Epics 1–2 | 🟢 |
| 4 | Reader extraction + sanitization pipeline | extracted content stored safely and opened from CLI/TUI | M | Epics 2–3 | 🟡 |
| 5 | TUI inbox and article reader | list/filter/open/read/archive/star works keyboard-first | L | Epics 2–4 | 🟡 |
| 6 | Search and filter layer | FTS5 search, source filters, unread/starred filters all work consistently | M | Epics 2–5 | 🟢 |
| 7 | OPML import + backup export | import feed sets and produce restoreable backups | M | Epics 2–3 | 🟢 |
| 8 | URL save / bookmark capture | `pergamon save` ingests a URL with dedupe and metadata extraction | M | Epics 2, 4, 6 | 🟡 |
| 9 | Collections, tags, and bulk actions | organize saved items and feeds efficiently in the TUI | M | Epics 5–8 | 🟡 |
| 10 | Fixture corpus + dogfood migration | real exports/feeds captured; maintainer can use pergamon daily for primary feeds | M | Epics 3–9 | 🟢 |

**Priority order:** 1 → 2 → 3 → 4 → 5 is the real MVP chain. Epics 7–10 can overlap once the schema and basic reader exist. **[Validated]**

### 14.3 Due Diligence Backlog

| Task | Priority | Why it matters | Status |
|---|---|---|---|
| Validate crates.io / GitHub namespace availability for `pergamon` | P0 | Name lock should happen early | **[Validation Required]** |
| Build a representative feed corpus (blogs, news, newsletters, podcasts, mixed encodings) | P0 | Prevent parser optimism | **[Validated]** |
| Test `readability` on real “bad pages” | P0 | Reader quality is a make-or-break wedge | **[Validation Required]** |
| Verify Netscape bookmark, Raindrop export, and Pinboard export field fidelity | P0 | Bookmark replacement credibility depends on this | **[Validation Required]** |
| Document Readwise export formats and edge cases | P1 | Needed for Phase 3 migration quality | **[Validation Required]** |
| Document Kindle My Clippings import constraints | P1 | Avoid promising a smoother path than exists | **[Validated]** |
| Validate Obsidian plugin packaging, update, and vault-permission patterns | P1 | Prevent plugin rework in Phase 4 | **[Validation Required]** |
| Decide web shell strategy (thin TS vs full Rust frontend) with a small prototype | P1 | Avoid web-stack churn later | **[Validation Required]** |
| Validate iOS share extension data handoff limits | P1 | Critical for practical mobile capture | **[Validation Required]** |
| Confirm AGPL operational implications for managed sync hosting | P2 | Needed before commercialization of sync | **[Validated]** |

### 14.4 ADR List (First 10)

| ADR | Title | Decision focus | Timing |
|---|---|---|---|
| ADR-001 | Workspace and crate boundaries | zero-I/O core, adapter responsibilities, crate layout | Week 1 |
| ADR-002 | Canonical item model | how feeds, bookmarks, articles, notes, and highlights share identity | Week 1 |
| ADR-003 | Feed polling and identity | ETag, Last-Modified, GUID fallback, dedupe | Week 2 |
| ADR-004 | Search and indexing | FTS5 scope, ranking, update strategy | Week 2 |
| ADR-005 | Reader extraction pipeline | readability, fallbacks, raw HTML retention | Week 2 |
| ADR-006 | Sanitization policy | `ammonia` allowlist and storage rules | Week 2 |
| ADR-007 | Bookmark identity | canonical URL rules, provenance, merge safety | Week 3 |
| ADR-008 | Export format | Markdown, JSON, backup, stable slugs/frontmatter | Week 4 |
| ADR-009 | Review model | highlight schema, FSRS usage, note linkage | Week 6 |
| ADR-010 | Obsidian contract | plugin scope, file structure, source-of-truth boundary | Week 8 |

---

## Section 15: Dependency Map

```text
Phase 0: First Feed
  ├─ ADR-001 Workspace boundary
  ├─ ADR-002 Canonical item model
  ├─ ADR-003 Feed polling + identity
  ├─ Feed parser + extraction pipeline
  └─ SQLite + FTS5 + TUI reader
          │
          ▼
Phase 1: Minimum Usable Reader
  ├─ OPML import
  ├─ Inbox / reader triage
  ├─ pergamon save <url>
  ├─ Search / filters
  └─ Backup export
          │
          ▼
Phase 2: Bookmark Manager
  ├─ Collections / tags
  ├─ Bookmark imports
  ├─ Metadata enrichment
  └─ Dedupe / merge policy
          │
          ▼
Phase 3: Knowledge Retention
  ├─ Highlight model
  ├─ FSRS review queue
  ├─ Kindle import
  └─ Readwise import
          │
          ▼
Phase 4: Obsidian & Polish
  ├─ Export schema
  ├─ Obsidian plugin
  ├─ Smart collections
  └─ Content rules
          │
          ├───────────────┐
          ▼               ▼
Phase 5: Web UI       Phase 6: iOS App
  ├─ WASM boundary     ├─ UniFFI bridge
  ├─ Axum web shell    ├─ SwiftUI app
  └─ Docker            └─ Share extension
          └───────────────┬───────────────┘
                          ▼
Phase 7: Sync & Multi-Device
  ├─ AGPL Axum sync server
  ├─ Device onboarding
  ├─ Conflict policy
  └─ Hosted + self-host deployment
                          │
                          ▼
Phase 8: Moonshots
  ├─ Browser extension
  ├─ AI-assisted retrieval
  ├─ Email digest
  └─ Cross-app bridges
```

**Critical path:**  
**Phase 0 → Phase 1 → Phase 2 → Phase 3 → Phase 4 → (Phase 5 and Phase 6 in parallel) → Phase 7**. **[Validated]**

**Why that path matters:**  
If pergamon cannot first win **single-device daily use**, sync and platform work become expensive ways to move an unproven product around. The product should be undeniably useful on one machine before it becomes available on every machine. **[Validated]**

**Parallelizable work**

- importer fixture collection
- documentation and ADRs
- Obsidian export prototyping
- WASM and UniFFI spikes
- CI/release engineering  
These can move alongside the main path without destabilizing it. **[Validated]**

---

## Section 16: Feasibility & Compromise Matrix

| Challenge | Ideal solution | Acceptable compromise | Cost of compromise | Recommendation |
|---|---|---|---|---|
| Reader extraction quality | reader mode works on most pages via `readability` | fall back to original URL or minimally cleaned content when extraction is weak | weaker “all in one” feel on edge cases | **Ship with fallback.** Do not build a custom extractor early. **[Validated]** |
| Bookmark dedupe | canonical URL + source provenance + safe merges | exact-URL dedupe only in MVP | more duplicate cleanup for users | **Start conservative, then expand.** **[Validated]** |
| Cross-platform parity | one Rust domain model across CLI/web/iOS | UI-specific behaviors diverge slightly while data model stays shared | some UX inconsistency | **Protect the model, tolerate shell differences.** **[Validated]** |
| Web stack simplicity | Rust core + thin TS shell | server-rendered admin + lighter client until WASM is ready | web UX is less ambitious early | **Prefer fewer moving parts.** **[Validation Required]** |
| Obsidian integration | stable, thin plugin using exports and APIs | export-only workflows before full plugin features | less seamless note insertion early | **File contract first.** **[Validated]** |
| Kindle migration | clean file import with preserved metadata | import text only, weaker context for some clippings | lower retention fidelity | **Support best-effort import, preserve raw source.** **[Validated]** |
| Readwise migration | near-full export fidelity | partial import of highlights + notes first | some cleanup on user side | **Good enough migration beats waiting for perfect.** **[Validation Required]** |
| iOS capture | share extension fully captures page + selection + metadata | capture URL immediately, enrich inside the app | extra user hop in some cases | **Stage ingestion if needed.** **[Validated]** |
| Sync correctness | typed merge policies and encrypted multi-device state | local-only and manual export/import until sync is mature | delayed multi-device adoption | **Delay sync rather than ship data loss.** **[Validated]** |
| AI integration | privacy-bounded on-device / BYO-model assist | defer AI entirely | less hype, more focus | **Defer unless it clearly compounds the owned corpus.** **[Validated]** |

**Decision:** pergamon should systematically choose **boring, trustworthy compromises** over impressive-but-fragile ones. Every compromise above preserves the product’s core story: owned library, durable exports, readable content, and no surprise lock-in. **[Validated]**

---

## Section 17: Naming

The name **pergamon** is already the right choice. **[Validated]**

A **pergamon** is a page, a leaf in a volume, and by extension a collected portfolio of material. That maps directly to the product: pergamon is where feeds, saved pages, bookmarks, highlights, and resurfaced knowledge accumulate into a personal corpus.

Why the name works:

- **Meaningful:** evokes pages, volumes, and personal curation.  
- **Terminal-friendly:** 5 characters; commands like `pergamon feed add`, `pergamon save`, and `pergamon review` feel crisp.  
- **Brand-fit:** it sounds like a serious knowledge tool, not a social app.  
- **Portfolio fit:** it sits naturally beside **ldgr**, **toku**, **kora**, and **tock** in the kafkade family.  
- **Scope-fit:** “pergamon” is broad enough to hold RSS, reading, highlights, and bookmarks without sounding tied to one narrow feature.  

**Decision:** keep the product name, brand name, and CLI binary name the same: **pergamon**. **[Validated]**

---

## Section 17A: Success Metrics

### 17A.1 Adoption Metrics

| Metric | Target | Why it matters | Status |
|---|---|---|---|
| Maintainer fully migrates primary feeds into pergamon | by end of Phase 1 | Dogfooding is the strongest early validation | **[Validated]** |
| Maintainer uses `pergamon` daily for 14 consecutive days | Phase 1 gate | Replaces aspirational roadmap progress with actual behavior | **[Validated]** |
| 100+ GitHub stars | by Phase 4 | Awareness signal, not vanity goal | **[Validation Required]** |
| 1,000+ release downloads | by Phase 5 | Indicates real curiosity beyond the maintainer circle | **[Validation Required]** |
| 25+ active Obsidian plugin users | by Phase 4/5 | Confirms the ingestion-to-notes boundary is valuable | **[Validation Required]** |
| 10+ paying managed sync users | by Phase 7 | Confirms optional hosting is commercially viable | **[Validation Required]** |

### 17A.2 Quality Metrics

| Metric | Target | Why it matters | Status |
|---|---|---|---|
| Feed sync success rate on fixture corpus | >99% | Ingestion reliability is foundational | **[Validated]** |
| Search latency on 100k items | <100 ms local | Search is a daily-use primitive | **[Validated]** |
| Article extraction acceptance | 80%+ of corpus | Reader mode must be credibly useful | **[Validation Required]** |
| Bookmark import fidelity | >98% for supported fields | Migration quality drives switching | **[Validation Required]** |
| Readwise / Kindle import fidelity | >95% for supported fields | Retention workflows depend on provenance accuracy | **[Validation Required]** |
| Sync data-loss incidents | 0 | Trust is destroyed by silent corruption | **[Validated]** |
| Export/restore round-trip | 100% for native backup format | “Your data is yours” must be demonstrable | **[Validated]** |

### 17A.3 Community Metrics

| Metric | Target | Why it matters | Status |
|---|---|---|---|
| First external issue with reproducible fixture | by Phase 2 | Shows real use, not passive stars | **[Validation Required]** |
| First external PR merged | by Phase 3 | Validates contributor model | **[Validation Required]** |
| 10+ fixture contributions or importer bug reports | by Phase 4 | Import-heavy products improve through real-world variance | **[Validation Required]** |
| ADR discussions resolved without architecture churn | 80%+ | Indicates docs are helping govern complexity | **[Validated]** |
| Sponsor count | 10+ recurring sponsors by Phase 5 | Signals sustainability without product distortion | **[Validation Required]** |

**North-star metric:**  
**“Could the maintainer uninstall or ignore Inoreader, Pocket/Reader, Raindrop, and Readwise for their primary daily workflow?”**  
If the answer is no, vanity metrics do not matter. **[Validated]**

---

## Section 18: Failure Mode Analysis

| # | Failure mode | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | **Trying to replace four products at once and shipping none** | High | Critical | Keep the roadmap sequential: win feeds/reading first, then bookmarks, then retention, then platforms, then sync. **[Validated]** |
| 2 | **Reader extraction quality is not good enough to displace incumbent readers** | Medium | High | Treat extraction as a gate in Phase 0; keep strong fallback behavior; collect a real fixture corpus early. **[Validation Required]** |
| 3 | **Bookmark and import fidelity are too weak to justify migration pain** | Medium | High | Preserve provenance, build dry-run previews, and test against real OPML/bookmark/Readwise exports. **[Validated]** |
| 4 | **Sync arrives too early and consumes the roadmap** | High | Critical | Make single-device value the gating criterion; do not start Phase 7 before daily use is proven across one-device flows. **[Validated]** |
| 5 | **Obsidian integration turns into a second product and destabilizes the source of truth** | Medium | High | Keep the plugin thin and export-oriented; pergamon remains the canonical ingestion store. **[Validated]** |
| 6 | **Web/iOS shells force bad compromises in `pergamon-core`** | Medium | High | Protect the zero-I/O boundary and stable domain API; let shell UX differ before the model diverges. **[Validated]** |
| 7 | **Open-source sustainability is weaker than expected** | Medium | Medium | Start with GitHub Sponsors; only add managed sync when support burden is justified; keep self-host and export strong. **[Validated]** |
| 8 | **AI/browser-extension moonshots distract from the trust-based wedge** | High | Medium | Keep Phase 8 explicitly deferred; no AI feature ships if it weakens the privacy/ownership story. **[Validated]** |

**Meta-risk:** the biggest threat is not technical impossibility; it is **loss of discipline**. pergamon is feasible if it behaves like a sharp product with a strong sequence, not like a category manifesto trying to ship in one release. **[Validated]**

---

## Section 19: Open Questions & Decision Log

> Anything marked **[Validation Required]** should be resolved before the phase it gates. Anything marked **Decided** should be treated as default unless new evidence is materially better.

| # | Decision | Recommendation | Why | Status |
|---|---|---|---|---|
| 1 | Core architecture boundary | **Zero-I/O `pergamon-core`; all HTTP/filesystem/platform I/O in adapters** | Preserves testability, WASM viability, and clean cross-platform reuse | **Decided [Validated]** |
| 2 | Canonical local store | **SQLite + FTS5** | Best fit for local-first, portable search-heavy workloads | **Decided [Validated]** |
| 3 | Feed parsing library | **`feed-rs`** | Fastest path to reliable normalized ingestion | **Decided [Validated]** |
| 4 | Reader extraction pipeline | **`readability` + raw-source fallback** | Good enough to ship quickly without custom extraction debt | **Decided [Validation Required]** |
| 5 | HTML sanitization | **`ammonia` allowlist** | Safe, boring, necessary infrastructure | **Decided [Validated]** |
| 6 | Terminal reader rendering | **`ratatui` + `termimad`** | Best current fit for a readable TUI experience | **Decided [Validation Required]** |
| 7 | Bookmark identity model | **raw URL + canonical URL + provenance metadata** | Enables safe dedupe without destructive merges | **Decided [Validated]** |
| 8 | Highlight data model | **source-agnostic highlight/note schema linked to canonical items** | Avoids per-source fragmentation later | **Decided [Validated]** |
| 9 | Review scheduler | **FSRS** | Modern retention model; better than inventing a custom heuristic | **Decided [Validated]** |
| 10 | Obsidian integration style | **export/plugin-first, not vault-as-source-of-truth** | Keeps pergamon as ingestion engine and avoids sync chaos | **Decided [Validated]** |
| 11 | Web app architecture | **Axum server + WASM core + thin TypeScript UI shell** | Balances Rust reuse with practical UI iteration speed | **Deferred to Phase 5 [Validation Required]** |
| 12 | iOS architecture | **SwiftUI consuming Rust via UniFFI** | Best blend of native UX and shared business logic | **Decided [Validated]** |
| 13 | Sync protocol | **encrypted blobs/events with typed merge policies; server never sees plaintext** | Matches local-first trust model while enabling optional hosting | **Deferred to Phase 7 [Validation Required]** |
| 14 | License / commercialization model | **Apache-2.0 for app/core, AGPL-3.0 for server, Sponsors + optional managed sync** | Maximizes openness while protecting the hosted server boundary | **Decided [Validated]** |
| 15 | Product and binary name | **pergamon** | Precise meaning, terminal-friendly, portfolio-fit inside kafkade | **Decided [Validated]** |

**Open questions that still deserve explicit validation**

- Is `readability` strong enough on the actual corpus pergamon users will save? **[Validation Required]**
- Does the web shell need more TypeScript than planned, or can the Rust/WASM boundary stay thin? **[Validation Required]**
- Can the iOS share extension ingest enough metadata to feel magical without building a background service? **[Validation Required]**
- How aggressive should sync auto-merge be for tags, notes, and read state before the product starts surprising users? **[Validation Required]**

**Closing decision:** pergamon should be built as a **disciplined local-first ingestion system**, not as a generalized PKM platform or a feature race against every incumbent at once. That is the roadmap’s core strategic choice. **[Validated]**
