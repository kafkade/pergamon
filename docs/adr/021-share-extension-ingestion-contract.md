# ADR-021: Share Extension Ingestion Contract

**Status**: Accepted  
**Date**: 2026-07-01  
**Deciders**: kafkade

## Context

Phase 6 ships a native SwiftUI iPhone app (epic #34) with a **share extension**
so a user can save a URL from Safari, or selected text from any app, straight
into pergamon from the iOS share sheet. The share extension is a separate
process from the main app, and iOS runs it under a hard budget: a small memory
ceiling (historically on the order of ~120 MB) and a short wall-clock window
before the system may terminate it once the sheet is dismissed. It is not a
place to fetch a page over the network, run readability extraction, sanitize
HTML, open the main SQLite database, or run schema migrations.

The rest of pergamon already has one ingestion pipeline, exercised by the CLI
`save` command (`crates/pergamon-cli`, `save_url`):

1. Fetch the URL (HTTP, redirect-following) — HTTP lives outside `pergamon-core`
   per ADR-007.
2. Canonicalize via `pergamon_extract::canonicalize_url` (strip tracking params,
   normalize scheme/host/port, drop fragment, sort query).
3. Dedupe via `get_content_item_by_url(canonical)` against the unique-URL index.
4. Create a `ContentItem` (`status = Inbox`, UUID id, UTC timestamps), attach
   `BookmarkMeta` (`saved_from`, `original_url`, favicon, site name, …).
5. Extract to an `Article` when readability succeeds, otherwise keep it a
   `Bookmark` — the same item upgrades in place per ADR-010 (progressive
   enrichment).

This ADR defines how the share extension feeds that **same** pipeline without
doing any of its expensive work inline. It decides three things the epic calls
out: the staging inbox format the extension writes, the finalization flow when
the main app next opens, and the conflict/dedupe rules — which must stay
identical to CLI and web ingestion.

This ADR **depends on ADR-020** (#111, mobile storage ownership and cache
policy), which owns the concrete on-device layout: the shared app-group
container, where the SQLite database and blob cache live, and who runs
migrations. ADR-020 is not yet accepted, so this ADR references those facilities
at the contract level (an app-group container the extension and app both reach;
the app owns the database and migrations) and defers concrete paths and size
limits to ADR-020. It also builds on ADR-007 (HTTP outside the core), ADR-010
(unified content model), ADR-016 (canonicalization and the state machine run in
native Rust, not a thin client), and ADR-019 (the UniFFI facade; epoch-millis
time mapping; a narrow `Library` handle the app drives).

## Decision

### The extension stages; it never finalizes

The share extension does the **minimum work to capture and hand off**, then
returns:

- **No network.** It does not fetch the shared URL.
- **No extraction or sanitization.** It does not run readability or ammonia.
- **No database access.** It does not open the main SQLite database and does not
  run migrations. This decouples the extension binary — which the system may run
  against a database written by a newer app build — from the schema version, and
  avoids cross-process write/migration races.
- **It writes exactly one staging record and exits.**

All fetching, extraction, canonicalization, dedupe, and database writes are
**deferred to the main app**, which runs them through the existing pipeline.

### Staging format: an append-only drop folder of atomic JSON records

The extension appends captures to a **drop folder** in the shared app-group
container (exact path owned by ADR-020), conceptually:

```text
<app-group-container>/staging/inbox/
    <capture_id>.json
    <capture_id>.json
    ...
```

- **One file per capture.** Each capture is an independent file, so a batch is
  never half-written and one bad record cannot corrupt others.
- **Atomic publish.** The extension writes to a temporary sibling
  (`<capture_id>.json.tmp`) and then `rename`s it to `<capture_id>.json`.
  Readers only ever see complete files, and a crash mid-write leaves only an
  ignorable `.tmp`.
- **Append-only from the extension's side.** The extension only ever creates new
  files; it never reads, mutates, or deletes existing ones. Only the main app
  deletes records, and only after they are durably ingested.

A drop folder — rather than direct SQLite inserts from the extension — is what
lets the extension stay ignorant of the schema, avoids migration/locking races
between two processes, and is trivially crash-safe.

### Staging record schema

Each record is a small JSON object. Fields:

| Field | Type | Req. | Meaning |
|-------|------|------|---------|
| `schema_version` | integer | yes | Staging format version; starts at `1`. Lets a newer app read older extension output and vice versa. |
| `capture_id` | string (UUID) | yes | Stable id for this capture; also the filename. Used for idempotent finalize. |
| `captured_at` | integer (epoch millis) | yes | Capture time. Epoch millis matches the ADR-019 time mapping. |
| `content_kind` | string enum | yes | `url`, `url_with_selection`, or `text` — a hint for finalization. |
| `url` | string | cond. | The raw shared URL (not yet canonicalized). Present for `url` / `url_with_selection`. |
| `selected_text` | string | cond. | The shared/selected text. Present for `url_with_selection` / `text`. |
| `page_title` | string | no | Title supplied by the share sheet (e.g. Safari), stored without a fetch. |
| `source_app` | string | no | Best-effort originating bundle id, for provenance. |

Forward compatibility: readers ignore unknown fields and tolerate a higher
`schema_version` by skipping records they cannot understand (leaving the file in
place) rather than dropping data. The extension keeps records small to respect
its memory budget; large selections are still just text and cost little.

### Finalization flow in the main app

On launch and on foreground, the app drains the drop folder. It processes
records **oldest-first by `captured_at`** and, for each record, runs the same
ingestion pipeline the CLI uses, driven through the ADR-019 `Library` handle:

1. **Canonicalize.** If `url` is present, compute
   `pergamon_extract::canonicalize_url(url)` — the exact function CLI and web
   use.
2. **Dedupe.** Look up `get_content_item_by_url(canonical)` against the shared
   unique-URL index.
3. **Create or reuse the item.** If no match, create a `ContentItem`
   (`status = Inbox`, `id = UUID v4`, UTC `created_at` / `updated_at`) and attach
   `BookmarkMeta` with `saved_from = "share-sheet"`, `original_url` = the raw
   shared `url`, and `page_title` as the title when extraction has not yet run.
   If a match exists, reuse it (see dedupe rules below).
4. **Attach the selection.** When `selected_text` is present alongside a URL,
   create a `Highlight` (`content_type = Highlight`, `HighlightMeta.quote_text =
   selected_text`, `source_item_id` = the URL item's id). A `text`-only capture
   becomes a **standalone** `Highlight` with `source_item_id = None`.
5. **Enqueue extraction.** For URL captures, enqueue a deferred fetch +
   readability pass in the app's orchestration layer (HTTP stays outside
   `pergamon-core`, ADR-007). The item is immediately usable as a bookmark and
   upgrades to `Article` in place when extraction later succeeds (ADR-010).
6. **Delete the staging file** only after the database write for that record has
   committed.

**Idempotency and crash-safety.** The unit of durability is the database commit,
not the file deletion. If the app crashes between step 5's commit and step 6's
delete, the surviving file is reprocessed on the next launch; because dedupe
keys on the canonical URL (and, for text-only highlights, on `capture_id`
provenance), reprocessing converges to the same item instead of duplicating it.
Malformed or future-versioned records are left in place and surfaced to
diagnostics rather than silently discarded.

### Dedupe and conflict rules

The share extension is just **another deferred producer feeding the one shared
ingestion function**; it introduces no new dedupe logic.

- **Canonical URL is the single dedupe key** across CLI, web, and mobile.
- **Merge on duplicate, do not create a second item.** When finalization finds
  an existing item for the canonical URL, it applies the capture's additions to
  that item — attaching the `selected_text` highlight and any tags — exactly as
  CLI `save` does when it hits an existing URL. `created_at` and the original
  `content_type`/content are preserved; enrichment only adds.
- **Text-only captures** have no URL and so are never URL-deduped; they always
  produce a standalone highlight. Their `capture_id` provides the provenance
  needed to avoid re-inserting the same record after a crash.

### Content-kind mapping

| `content_kind` | Result |
|----------------|--------|
| `url` | One `ContentItem` (Bookmark, later Article via deferred extraction). |
| `url_with_selection` | The URL `ContentItem` **plus** a `Highlight` linked by `source_item_id`. |
| `text` | A standalone `Highlight` (`source_item_id = None`). |

## Consequences

### Positive

- The extension stays within its memory/time budget: it only serializes a small
  JSON record and renames a file, with no network, extraction, or database work.
- The extension binary is decoupled from the database schema version; only the
  app runs migrations (ADR-020), eliminating cross-process migration and write
  races.
- Atomic temp-write + rename and one-file-per-capture make staging crash-safe;
  no partial or interleaved writes are ever visible.
- Mobile ingestion reuses the exact CLI/web pipeline (canonicalize → dedupe →
  create/enrich → extract), so dedupe and enrichment behavior are identical
  across surfaces by construction.
- Deferring extraction means a capture succeeds instantly even offline or on a
  flaky connection; the article fills in later when the app can fetch.
- Selected text is preserved as a first-class highlight tied to its source,
  matching the existing highlight model.

### Negative

- A newly shared item is only a bookmark until the app next opens and runs
  deferred extraction; there is a visible lag before article text and richer
  metadata appear.
- Correctness depends on the finalize step being idempotent; the commit-then-
  delete ordering and canonical-URL dedupe must be honored or duplicates/loss
  become possible.
- The drop folder can accumulate if the app is not opened for a long time; the
  app must drain it eagerly and bound its growth (limits deferred to ADR-020).
- Two `schema_version`s must be tolerated during an app/extension upgrade skew
  window, adding a small compatibility burden to the reader.
- Provenance for text-only captures leans on `capture_id`; without it, repeated
  finalize of a surviving file could duplicate a standalone highlight.

## Rejected Alternatives

- **Extension writes directly into the shared SQLite database.** Rejected: it
  forces the extension to know the schema and possibly run migrations, invites
  cross-process write/lock/migration races against a database the newer app
  owns, and makes crash-safety much harder than an atomic file rename. The drop
  folder keeps the extension schema-agnostic.
- **Extension performs the full fetch + extraction and writes finished items.**
  Rejected: network fetch and readability blow the share-extension memory/time
  budget and fail entirely offline. Deferring to the app is both safer and more
  reliable, and keeps HTTP outside `pergamon-core` per ADR-007.
- **A single append-only log file instead of one file per capture.** Rejected:
  concurrent appends and partial writes across a process boundary are error-
  prone, and truncating consumed prefixes is fiddly. Independent files give
  atomic publish and trivial per-record deletion.
- **Push finalize immediately via a Darwin notification from the extension.**
  Rejected as the contract's basis: the main app may not be running and the
  extension must not assume it can wake it. Scan-on-open is the reliable
  baseline; a notification may later be an optimization on top of it, not a
  replacement.
