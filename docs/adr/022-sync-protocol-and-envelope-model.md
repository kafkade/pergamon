# ADR-022: Sync Protocol and Envelope Model

**Status**: Accepted  
**Date**: 2026-07-02  
**Deciders**: kafkade

## Context

Phase 7 (epic #35) adds **optional, end-to-end encrypted multi-device sync**
through an AGPL-3.0 Axum server (`pergamon-sync-server`) without breaking the
local-first trust model. The roadmap (§2.5) already fixes the shape: local
SQLite is the source of truth, sync is a thin replication layer over canonical
local state, every local mutation writes both canonical tables and an outbox row
in the same transaction, and **the server stores ciphertext only — it never sees
plaintext**.

This ADR defines the **wire contract** that makes that possible, and nothing
more. Concretely it decides four things the epic calls out for #121:

1. The **envelope schema** — what a synced change looks like on the wire,
   including which fields the server may read and index and which are opaque
   ciphertext.
2. The **event-log vs blob-store split** on the server.
3. **Push/pull cursor semantics and idempotency**.
4. The **wire format and versioning** strategy.

It deliberately does **not** decide:

- **Conflict resolution semantics** — how two concurrent edits to the same
  entity are reconciled (LWW, set-union, conflict copies, tombstones). That is
  ADR-023 (#122). This ADR only carries the metadata (version/clock) that
  ADR-023 will consume.
- **The concrete cryptography** — cipher suite, key derivation, device key
  exchange, and account bootstrap. Those are ADR-024 (#123) and the E2E
  encryption work (#125). This ADR defines the envelope's crypto-relevant
  *fields* (ciphertext, nonce, AAD, key epoch reference) and the invariant that
  the server only ever holds ciphertext; it defers *which* AEAD and *how* keys
  are managed.

### Dependencies and constraints

- **ADR-008 (licensing):** `pergamon-sync-server` is AGPL-3.0 and lives in its
  own crate; the envelope types it exchanges are defined in the Apache-2.0
  client side and serialized on the wire, so no server code leaks into
  `pergamon-core`. The server understands the *frame*, never the *payload*.
- **ADR-007 (platform boundaries):** `pergamon-core` stays zero-I/O. Envelope
  encoding/decoding and the change-tracking outbox model are pure and testable
  in the core/client crates; HTTP and storage live in the client sync engine
  (#126) and the server.
- **ADR-006 (schema):** entities already carry stable UUID identities and
  `updated_at` timestamps; the outbox is added as sync infrastructure over that
  canonical schema.
- **§2.5 / §5.2 (sync scope):** sync covers durable knowledge artifacts
  (documents with annotations, starred/archived items, tags, collections,
  review state), not the transient unread firehose. The protocol is
  scope-agnostic — the client decides which entities produce outbox rows — but
  the split below (small events, large blobs) is sized for that profile.

## Decision

### Two server stores: an append-only event log and a content-addressed blob store

The server keeps exactly two per-account stores, and understands the structure
of neither payload:

```text
pergamon-sync-server (per account)
├── event log        append-only, server-sequenced stream of encrypted envelopes
│                     (small: metadata mutations, annotations, review events)
└── blob store        content-addressed by client-supplied ciphertext hash
                      (large / immutable: HTML snapshots, PDFs, extracted text,
                       encrypted snapshots)
```

- **The event log is the ordering spine.** Every logical change is one encrypted
  **event envelope** appended to the account's log. The server assigns each a
  strictly monotonic `server_seq` on commit; that sequence is the only thing
  cursors are expressed in.
- **The blob store holds bulk, immutable content out of band.** A blob is
  addressed by the hash of its *ciphertext* (`ct_hash`). Blobs are uploaded
  first and referenced by hash from events, so the log stays small and an event
  that references a blob is only durable once the blob it points at exists
  (upload-before-commit; see push).
- **Why split them:** events must be totally ordered, cheap to fetch
  incrementally, and cheap to retain; blobs are large, immutable, and naturally
  deduplicated by hash. Interleaving multi-megabyte snapshots into the ordering
  stream would make incremental pull expensive and retention hard. This mirrors
  §2.5's "encrypted batches and encrypted blob chunks" split and keeps the
  server's job to "append events, store blobs by hash, serve cursors, prune".

### The event envelope

An event envelope has a small, deliberately **content-free server header** and
an **opaque encrypted body**. The server reads and may index the header; it can
never read the body.

**Server-visible header** (plaintext on the wire, indexed by the server):

| Field | Type | Meaning |
|-------|------|---------|
| `protocol_version` | integer | Wire protocol major version (starts at `1`). |
| `account_id` | string (opaque) | Which account's log this belongs to. Established at onboarding (ADR-024); an opaque handle, not an email/identity. |
| `device_id` | string (opaque) | Origin device, so a device can suppress echoes of its own events on pull. Opaque per-device handle. |
| `change_id` | string (UUID) | Client-generated, globally unique id for this change. The **idempotency key** for push and the client-side dedupe key on pull. |
| `entity_ref` | string (opaque, optional) | A **blinded** per-entity grouping token: `HMAC(account_stream_key, entity_type‖entity_id)`. Lets the server coalesce/compact an entity's history and lets a client request "just this entity" without the server learning the entity's real id or type. Absent for events not tied to a single entity. |
| `key_epoch` | integer | Which account key epoch encrypted the body, so clients can decrypt across key rotations (rotation itself is ADR-024). |
| `blob_refs` | array of string (`ct_hash`) | Ciphertext hashes of blobs this event depends on. Enables upload-before-commit and reference-counted blob GC. |
| `payload_bytes` | integer | Size of the ciphertext, for quotas and batching. |
| `server_seq` | integer | **Assigned by the server** on commit; strictly monotonic per account. The cursor domain. Not set by the client. |
| `server_committed_at` | integer (epoch millis) | Server receive time, for retention/pruning only — never used for ordering or conflict logic. |

**Encrypted body** (AEAD ciphertext; opaque to the server). After decryption the
client sees the semantic change:

| Field | Type | Meaning |
|-------|------|---------|
| `entity_type` | string enum | `document`, `annotation`, `tag`, `collection`, `feed_subscription`, `review_card`, `review_event`, `settings`, … |
| `entity_id` | string (UUID) | The stable domain id (ADR-006). |
| `op` | string enum | `upsert`, `delete` (tombstone), or `field_patch`. |
| `clock` | HLC | A **hybrid logical clock** stamp (`{wall_millis, counter, device_id}`) for this change, plus the entity's prior `version` the writer observed. Provides a total, causally-consistent order that ADR-023 uses to detect and resolve concurrency. This ADR only *transports* it. |
| `fields` | object | The changed fields / new entity state (per-field for `field_patch`, enabling ADR-023's per-field LWW). |
| `blob_manifest` | array | For each `blob_ref` in the header: `{ ct_hash, role, plaintext_len, chunk_layout }`, so the client can locate and decrypt the referenced content. |

The **AEAD associated data (AAD)** binds the ciphertext to the header fields that
must not be tampered with in transit — at minimum `protocol_version`,
`account_id`, `change_id`, `key_epoch`, and the `blob_refs` list — so a
malicious or buggy server cannot re-target, replay across accounts, or swap the
blobs of an event without decryption failing on the client. The nonce is carried
alongside the ciphertext in the encrypted-body frame. The specific AEAD
construction and key schedule are ADR-024/#125; this ADR requires only that it be
a modern AEAD and that the AAD cover those fields.

**Why identity lives inside the ciphertext.** `entity_type`, `entity_id`, the
clock, and the field changes are all in the encrypted body. The server orders
and prunes purely on `server_seq`, deduplicates on `change_id`, attributes
origin via `device_id`, and — only when the client opts in — coalesces history
per `entity_ref`. The blinded `entity_ref` gives useful server-side compaction
and targeted fetch **without** revealing what the entity is, honoring "the
server never sees plaintext" while still exposing "metadata the server may
index".

### Blob envelope

A blob is bulk content stored out of band and referenced by events.

- **Content-addressed by ciphertext hash.** Address = `ct_hash` = hash of the
  encrypted bytes. Because pergamon encrypts client-side and (per #125) intends
  content-derived / convergent keys for immutable content, identical plaintext
  encrypts to identical ciphertext and therefore **deduplicates on the server by
  hash** — matching §2.5's "dedup by hash; no logical conflict" for blobs.
- **Immutable.** A blob is never mutated; a changed snapshot is a new blob with a
  new hash, referenced by a new event. This makes blob conflicts impossible by
  construction.
- **Chunked and encrypted client-side.** Large blobs are split into fixed-size
  chunks, each encrypted; the `chunk_layout` in the event's `blob_manifest`
  records how to reassemble. The server stores opaque chunks keyed by hash and
  never concatenates or interprets them.
- **Reference-counted GC.** A blob is retained while any live (non-pruned) event
  references its `ct_hash`; it becomes collectable when its last referencing
  event is pruned. The server computes this over header `blob_refs` only — no
  payload access needed.

### Push semantics and idempotency

Push is **upload blobs, then append events**, and is safe to retry blindly.

1. **Upload missing blobs.** The client asks which of a set of `ct_hash`es the
   server already has (dedup probe), then uploads only the missing chunks. Blob
   upload is idempotent: re-uploading an existing `ct_hash` is a no-op because
   the address *is* the content.
2. **Append events.** The client submits a batch of event envelopes. For each,
   the server:
   - **Rejects** (does not append) any event whose header `blob_refs` are not
     all already present in the blob store — enforcing upload-before-commit, so
     the log never contains a dangling reference.
   - **Deduplicates on `change_id`.** If an event with that `change_id` is
     already in the log, the server does not append a second copy; it returns
     the existing `server_seq`. This makes the whole push idempotent under
     retry, at-least-once delivery, and mid-batch crashes.
   - Otherwise **appends**, assigns the next `server_seq`, and returns it.
3. **Batch result.** The server returns, per `change_id`, its assigned (or
   pre-existing) `server_seq`, plus the account's current high-water
   `server_seq`. A batch is accepted independently of overlaps with earlier
   batches, so partial failures are retried by simply resending — accepted
   events dedupe, only genuinely new ones append.

The unit of client durability is the local commit of the outbox row, not the
network call. An outbox row stays pending until the server acknowledges its
`change_id`; because acknowledgment is idempotent, a client that crashes between
"server appended" and "client marked outbox row acked" simply re-pushes and gets
the same `server_seq` back.

### Pull semantics and cursors

Pull is a **cursor-based scan of the append-only event log**.

- **A cursor is a single `server_seq` high-water mark.** A client persists "the
  greatest `server_seq` I have durably applied". To pull, it asks for events with
  `server_seq > cursor`, in ascending `server_seq` order, in pages.
- **Monotonic and stable.** `server_seq` is assigned once at append and never
  changes or is reused, so the cursor is a durable, resumable position. Pulling
  is idempotent: re-requesting from the same cursor yields the same events in the
  same order.
- **Echo suppression.** The client's own events come back on pull (they are in
  the shared log). The client skips any event whose `device_id` is its own —
  it already applied that change locally — while still advancing its cursor past
  them. `change_id` is the secondary guard: applying is idempotent because an
  already-applied `change_id`/`entity_id@clock` is a no-op.
- **Apply, then advance.** The client decrypts each event, applies it into local
  SQLite through the same domain operations local edits use (resolving
  concurrency per ADR-023), and only then advances its persisted cursor past that
  `server_seq`. A crash mid-page re-pulls from the last durably-advanced cursor;
  re-application is idempotent, so no change is lost or double-applied.
- **Blobs pulled on demand.** Applying an event fetches any referenced `ct_hash`
  blobs the device does not already have, then decrypts them via the
  `blob_manifest`. A device may lazily defer large-blob fetches (e.g. full
  extracted text) per its sync profile (§5.2) without blocking metadata apply.

### Snapshots for fast onboarding

To avoid replaying years of history event-by-event on a new device (§2.5), the
protocol supports **encrypted snapshots**:

- A snapshot is an **encrypted blob** (same blob store, same content-addressing)
  containing canonical state — documents, tags/collections, annotations, review
  state — as of a specific `server_seq` watermark.
- Snapshots are referenced by a small **snapshot-manifest event** in the log
  (just another envelope: header names the snapshot blob and its
  `server_seq` watermark; body holds the decryption manifest).
- A new device bootstraps by fetching the latest snapshot, applying it, setting
  its cursor to the snapshot's watermark, and then pulling incrementally from
  there. The server still cannot read snapshots — they are ordinary ciphertext
  blobs — and produces them only by storing what a client uploads; snapshot
  *creation* is a client responsibility.

### Wire format and versioning

- **Transport:** HTTPS to the Axum server, request/response bodies encoded as a
  compact self-describing binary format (the same serde-based encoding the
  client already uses), gzip-compressed for event batches (§2.5 "compressed and
  encrypted client-side" — compression is applied to *plaintext before
  encryption* inside the body; the server sees only ciphertext).
- **Three independent version numbers, versioned separately so they can evolve
  without lockstep:**
  1. `protocol_version` (header) — the **transport/endpoint contract** (routes,
     batch framing, cursor semantics). Bumped only on breaking wire changes. The
     server advertises supported protocol versions; a client negotiates the
     highest common one.
  2. **Envelope `body_schema_version`** (first field inside the ciphertext) —
     the **semantic payload shape** (`entity_type` set, `fields`, `clock`
     format). Because it is inside the body, only clients — never the server —
     need to understand it, so it can evolve independently of the server.
  3. **`key_epoch`** (header) — the **crypto epoch** for key rotation
     (semantics owned by ADR-024).
- **Forward/backward compatibility:** decoders ignore unknown fields and tolerate
  a higher `body_schema_version` by skipping events they cannot fully interpret
  (leaving the cursor un-advanced past them only if application would lose data;
  otherwise recording them for later) rather than corrupting local state. New
  `entity_type`s are additive. The server, understanding only the header, is
  unaffected by body-schema evolution entirely.

## Consequences

### Positive

- **The server is a blind, append-only ordering and storage service.** It sees
  opaque account/device handles, a monotonic sequence, idempotency keys, blob
  hashes, sizes, and a blinded per-entity token — never plaintext, entity ids,
  or types. "The server never sees plaintext" is enforced by construction, and
  AAD binding stops it from tampering with routing.
- **Idempotency is end to end.** `change_id` dedupe on push, `server_seq`
  cursors on pull, content-addressed blobs, and idempotent local apply mean
  every operation is safe to retry after any crash or partial delivery — no
  duplicates, no lost changes.
- **Event/blob split keeps sync cheap.** Incremental pull moves small events;
  bulk immutable content deduplicates by hash and is fetched lazily per profile,
  so a large library still syncs metadata quickly.
- **Clean layering.** The envelope transports version/clock but does not resolve
  conflicts (ADR-023) and carries crypto fields but does not manage keys
  (ADR-024/#125), so those decisions can land independently against a stable
  wire contract.
- **License boundary stays clean (ADR-008).** The AGPL server only ever handles
  the frame; all payload semantics live in the Apache-2.0 client, and no server
  logic is pulled into `pergamon-core` (ADR-007).
- **Independent version axes** let the transport, the payload schema, and the
  crypto epoch each evolve without forcing a coordinated flag-day upgrade.

### Negative

- **Server-side features are limited by blindness.** The server cannot do
  content search, server-side merge, dedupe by URL, or per-field compaction of
  what it cannot read; the richest it can do is per-`entity_ref` history
  coalescing. This is the intended trade for E2EE and is consistent with §2.5's
  "server should not run FTS / dedupe content".
- **Clients carry the hard work.** Encryption, decryption, snapshotting,
  conflict resolution, and blob chunking all run client-side, making the sync
  engine (#126) substantially more complex than a plaintext REST client.
- **The blinded `entity_ref` leaks coarse activity shape.** The server learns how
  many events touch the "same" opaque token and their sizes/timing, which is a
  small metadata side channel even though identities and contents stay hidden.
  Clients that want to minimize it can omit `entity_ref`, losing server-side
  per-entity compaction and targeted fetch.
- **Upload-before-commit adds a round trip.** An event referencing new blobs
  cannot be appended until its blobs land, so a push may need a blob-probe /
  upload phase before the event phase. This is the price of never having a
  dangling reference in the log.
- **Retention/GC must be reference-counted carefully.** Pruning acknowledged
  events must not orphan a blob still referenced by a live snapshot or a
  laggard device's un-pulled history; the server must track references and
  device cursors before collecting.

## Rejected Alternatives

- **Server-visible entity ids and field diffs (plaintext or server-decryptable).**
  Rejected: it lets the server index and merge, but breaks the core E2EE
  guarantee that the server never sees plaintext. The blinded `entity_ref`
  recovers most of the useful server-side grouping without exposing identity.
- **One interleaved stream for events and blobs (no split).** Rejected:
  multi-megabyte snapshots in the ordering stream make incremental pull and
  retention expensive, and forfeit hash-based blob dedup. A separate
  content-addressed blob store is cheaper and simpler to GC.
- **Timestamp- or offset-based cursors.** Rejected: wall-clock timestamps are
  non-monotonic across devices and collide; byte offsets are fragile under
  compaction. A server-assigned monotonic `server_seq` is a stable, resumable,
  collision-free cursor.
- **Sequence numbers as the idempotency key.** Rejected: the client cannot know
  its `server_seq` before the server assigns it, so it cannot use it to make a
  push retry-safe. A client-generated `change_id` is known before the first
  attempt and survives retries, which is exactly what idempotent push needs.
- **Full event sourcing with no snapshots.** Rejected: replaying the entire
  history to onboard a device scales poorly as the library grows. Encrypted
  snapshots plus incremental events bound onboarding cost (§2.5).
- **Mutable, path-addressed blobs.** Rejected: mutable blobs reintroduce blob
  conflicts and cache-invalidation and defeat cross-device dedup.
  Content-addressing by ciphertext hash makes blobs immutable, deduplicated,
  and conflict-free by construction.
- **Deciding conflict resolution and the cipher suite here.** Rejected as scope:
  folding ADR-023 (conflict policy) and ADR-024/#125 (keys and crypto) into this
  ADR would couple three independently evolving concerns. This ADR fixes only
  the envelope, stores, cursors, and versioning they all build on.
