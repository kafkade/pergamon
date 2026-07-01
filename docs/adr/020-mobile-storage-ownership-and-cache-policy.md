# ADR-020: Mobile Storage Ownership and Cache Policy

**Status**: Accepted  
**Date**: 2026-07-01  
**Deciders**: kafkade

## Context

Phase 6 delivers a native SwiftUI iPhone app that reuses `pergamon-core` through
UniFFI (epic #34). ADR-019 (#110) fixed the FFI boundary: Swift reaches the core
only through the `pergamon-uniffi` facade, and a stateful `Library` handle wraps
the on-device SQLite store. What ADR-019 deliberately left open is *where that
store physically lives on iOS and how it is managed under the platform's
constraints*. This ADR decides that.

The iOS app is not a single process. Alongside the main app it ships a **share
extension** (#119) so a user can save a URL or selection from Safari without
launching pergamon. On Apple platforms an app extension runs in its own process
with its own sandbox; by default it cannot see the main app's container. Both
processes must nonetheless read and write **one** canonical library — a URL
saved from the share sheet has to appear in the app's inbox, and the app's
triage state has to be visible to future captures. That forces an explicit
decision about container ownership.

The library also has two very different kinds of data, established by ADR-006 and
the roadmap storage model (roadmap §2.3):

- **Small, precious, structured state** — metadata, normalized/extracted text,
  annotations, highlights, and FSRS review cards — living in SQLite (with FTS5).
  This is the canonical, hard-to-reconstruct part of a user's library.
- **Large, immutable, mostly reconstructable binaries** — raw HTML snapshots,
  PDFs, and other captured assets — living in a **content-addressed blob store**
  on disk, named by SHA-256 hash.

On a laptop these coexist happily inside a managed multi-gigabyte budget
(ADR-006: ~5 GB default). A phone is different: storage is scarce, the OS
reclaims space aggressively, and iCloud backup bandwidth and quota are precious.
The roadmap already prescribes the mobile posture (roadmap §2.3): *iOS defaults
to metadata + recent text + selectively cached blobs, and aggressively manages
old raw assets.* This ADR turns that guidance into concrete ownership, cache,
backup, and migration policy.

The issue (#111) scopes four questions:

1. **DB location** — app container vs. App Group — so the share extension and the
   main app share state.
2. **Blob / raw-HTML cache policy and size limits** on device.
3. **Backup / iCloud exclusion** decisions for large caches.
4. **Migration handling** on app upgrade.

This ADR builds on ADR-006 (schema, FTS5, blob budget, and the retention policy
classes), ADR-007 (SQLite + app-sandbox blobs on iOS; only the orchestration
layer performs HTTP), and ADR-019 (the `Library` handle owns the on-device
store). It is a documentation decision; the reference implementation lands with
the offline-database work (#118) and the share extension (#119). The
share-extension *ingestion handshake* itself is owned by ADR-021 (#112) — this
ADR pins only the storage-layer invariants that handshake depends on.

## Decision

### 1. Ownership and location — a shared App Group container

The canonical SQLite database and the content-addressed blob directory live in a
shared **App Group** container (`group.<bundle-id>`), obtained via
`FileManager.containerURL(forSecurityApplicationGroupIdentifier:)`, **not** in
either process's private app container. The App Group is the single storage
authority; both the main app and the share extension mount the same directory.

The on-device layout mirrors the desktop content-addressed scheme so the core's
storage code is platform-agnostic:

```text
<AppGroup>/Library/Application Support/pergamon/
├── pergamon.db            # SQLite: metadata, extracted text, FTS5, annotations, cards
├── pergamon.db-wal        # SQLite write-ahead log (transient)
├── pergamon.db-shm        # SQLite shared-memory index (transient)
└── blobs/                 # content-addressed raw assets, sharded by hash prefix
    └── ab/
        └── abcd…ef         # <sha256> raw HTML / PDF / MIME blob
```

There is exactly one `pergamon.db` and one `blobs/` tree per install. The
`Library/Application Support` placement (not `Documents`, not `Caches`) is
deliberate and interacts with the backup policy in decision 4: it is
app-managed, non-user-facing storage that the system does not purge arbitrarily,
while still letting us exclude the blob subtree from backup explicitly.

### 2. Cross-process concurrency

Because two processes open the same SQLite file, the store runs in **WAL
(write-ahead logging) mode** with a bounded `busy_timeout`. WAL allows concurrent
readers alongside a single writer and is the standard mode for multi-process
SQLite access within an App Group.

- **Single logical writer, short transactions.** SQLite still serializes
  writers; `busy_timeout` makes a blocked writer wait briefly rather than fail
  immediately with `SQLITE_BUSY`.
- **The share extension writes small and fast.** Its job is a bounded capture —
  insert the document row (and enqueue any follow-up work) and return. It does
  **not** fetch, extract, or run migrations. Heavy extraction and blob
  materialization happen later in the main app, consistent with ADR-007 (only
  the orchestration layer does HTTP; capture crates take bytes, they do not
  reach out). This keeps the extension well under the memory and wall-clock
  budgets Apple imposes on extensions.
- **WAL sidecars live beside the DB** in the App Group and are treated as
  transient (decision 4 excludes them from backup).

The full capture/enqueue contract between the extension and the app — payload
shape, deduplication, and hand-off of extraction — is owned by ADR-021 (#112).
This ADR fixes only the storage invariants that contract relies on: one shared
WAL-mode database in the App Group, extension writes are small and bounded, and
the main app owns schema migration (decision 5).

### 3. Blob cache policy and size limits

The blob store on iOS is a **bounded cache**, not an unbounded archive. It reuses
the retention policy classes defined in ADR-006:

- `pinned` — never auto-evicted.
- `keep-original` — retained unless the user prunes.
- `reconstructable` — safe to re-fetch/re-derive if needed.
- `cache-only` — freely evictable under budget pressure.

Policy on device:

- **The DB is never a cache.** Metadata, normalized/extracted text, annotations,
  highlights, and review cards always stay in `pergamon.db` and are never
  evicted by cache management. Reading and reviewing therefore work offline even
  after every raw blob has been reclaimed.
- **Raw blobs default to evictable.** Captured raw HTML and re-fetchable assets
  default to `reconstructable` / `cache-only`. PDFs and other assets the user
  explicitly saves default to `keep-original`; the user (or a future sync
  policy) may `pin` specific items.
- **Small, user-adjustable budget.** iOS ships with a small default blob budget
  (well below the desktop ~5 GB), surfaced as a user-adjustable setting. The
  desktop budget is not inherited.
- **LRU eviction under pressure.** When usage crosses the budget high-watermark,
  or the OS signals low storage / device pressure, the cache evicts
  least-recently-used `cache-only` then `reconstructable` blobs until back under
  budget. `pinned` and `keep-original` blobs are never auto-evicted; if only
  protected blobs remain and the budget is still exceeded, the app surfaces the
  condition rather than deleting protected data.
- **Eviction is metadata-preserving.** Evicting a blob removes the on-disk file
  only; the DB row, its content hash, and its source URL remain, so the asset can
  be re-fetched or re-derived on demand and the library stays complete.

### 4. iCloud / backup exclusion

The two data classes get opposite backup treatment:

- **The SQLite database is backed up.** `pergamon.db` is small, precious, and
  expensive to reconstruct (annotations and review scheduling cannot be
  re-derived from the network). It is **included** in iCloud / device backup.
- **The blob cache is excluded from backup.** The `blobs/` directory is large and
  largely reconstructable, so it is **excluded** by setting
  `URLResourceValues.isExcludedFromBackup = true` on the `blobs/` directory URL
  when the store is initialized. This keeps backups small and fast and avoids
  spending the user's iCloud quota on regenerable bytes.
- **WAL sidecars are excluded.** `pergamon.db-wal` and `pergamon.db-shm` are
  transient; they are excluded from backup, and the app checkpoints WAL into the
  main database at appropriate lifecycle points so a backup of `pergamon.db`
  alone is consistent.

This split — back up the canonical DB, exclude the reconstructable cache — is the
direct mobile expression of ADR-006's "metadata + text in SQLite, raw binaries in
a content-addressed blob store."

### 5. Migration handling on app upgrade

Schema evolution reuses the existing embedded migration runner from ADR-006 /
`pergamon-storage`; iOS does not get a second migration mechanism.

- **Stable path across upgrades.** The App Group container path is stable across
  app updates, so an upgrade opens the same `pergamon.db` and `blobs/` tree the
  previous version wrote. No data moves on upgrade.
- **The main app owns migration.** Schema migrations run on the main app's first
  launch after an update, inside a transaction, before the UI reads the store.
  The **share extension never migrates**: if it launches against a
  newer-on-disk / older-binary situation or an un-migrated database, it takes the
  conservative path (defer/queue the capture) rather than mutating schema from a
  constrained extension process. This keeps migration single-owner and avoids two
  processes racing a schema change.
- **Versioned, rebuildable blob layout.** The blob directory layout is versioned.
  Because blobs are content-addressed and (mostly) reconstructable, a future
  blob-format or sharding change can rebuild or lazily re-shard the cache rather
  than requiring a fragile in-place data migration; worst case the cache is
  cleared and repopulated on demand.

## Consequences

### Positive

- One App Group container gives the main app and the share extension a single
  shared library with no copy/sync-between-sandboxes machinery.
- WAL mode plus a small, bounded extension write keeps concurrent access safe and
  fits Apple's extension resource limits.
- Reusing ADR-006's retention classes means the same policy vocabulary describes
  desktop and mobile storage; only the budget and eviction aggressiveness differ.
- Backing up the DB while excluding the blob cache keeps iCloud backups small and
  fast without risking annotations or review scheduling.
- Metadata-preserving eviction means the library stays complete and offline
  reading/review keeps working even after raw blobs are reclaimed.
- Reusing the existing migration runner avoids a bespoke iOS migration path and
  keeps schema evolution single-owner.

### Negative

- App Group entitlements and a shared container add project configuration and a
  correctly-scoped `group.<bundle-id>` that both targets must share.
- Multi-process SQLite requires disciplined WAL handling and checkpointing;
  careless long writes in either process can still contend.
- A small default blob budget means users on cellular / offline may hit cache
  misses and need a re-fetch (which requires connectivity), unlike the desktop's
  generous local archive.
- Excluding the blob cache from backup means a device restore rebuilds blobs on
  demand rather than restoring them instantly.
- The "extension never migrates" rule means a capture taken immediately after an
  update but before the main app has launched-and-migrated is deferred rather
  than written straight through.

## Rejected Alternatives

- **Per-app-container database (no App Group):** rejected because the share
  extension and main app would each own a separate store, forcing an
  export/import or copy step between sandboxes and breaking the "one canonical
  library" model. The App Group is the standard Apple mechanism for exactly this
  sharing.
- **Storing raw blobs inside SQLite on mobile:** rejected for the same reasons as
  ADR-006 on desktop — it bloats the database, and here it would additionally
  drag large reconstructable binaries into the iCloud-backed DB, defeating the
  backup-exclusion strategy. Blobs stay content-addressed on disk.
- **Backing up the whole store (DB + blob cache) to iCloud:** rejected because it
  wastes the user's backup quota and bandwidth on regenerable bytes; the cache is
  explicitly excluded while the precious DB is preserved.
- **No local blob cache (always re-fetch on demand):** rejected because it breaks
  offline reading — a core local-first promise — and makes the reader dependent
  on connectivity for content already captured. A bounded, evictable cache is the
  compromise.
- **Letting the share extension run extraction and migrations:** rejected because
  extensions are memory- and time-constrained and run in a separate process;
  fetching/extracting there duplicates network logic (violating ADR-007) and
  racing schema migrations across two processes is unsafe. The extension captures
  small and bounded; the main app owns extraction and migration.
- **An unbounded on-device archive mirroring the desktop budget:** rejected
  because phone storage is scarce and OS-managed; inheriting the ~5 GB desktop
  budget would make pergamon a poor citizen under storage pressure. Mobile
  defaults to a small, user-adjustable, aggressively managed cache.
