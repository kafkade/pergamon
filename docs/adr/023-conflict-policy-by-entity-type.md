# ADR-023: Conflict Policy by Entity Type

**Status**: Accepted  
**Date**: 2026-07-02  
**Deciders**: kafkade

## Context

Phase 4 (epic #35) adds optional multi-device sync. ADR-022 (#121) fixed the
**wire contract**: an append-only, server-sequenced log of encrypted event
envelopes, content-addressed blobs, and cursor-based pull. It deliberately did
**not** decide how two concurrent edits to the same entity are reconciled — it
only *transports* the metadata that reconciliation needs. Concretely, every
decrypted event body carries (ADR-022):

- `entity_type`, `entity_id`, and `op` (`upsert` / `delete` / `field_patch`),
- a **hybrid logical clock** (`clock = {wall_millis, counter, device_id}`) plus
  the prior `version` the writer observed, and
- `fields` — per-field changes for `field_patch`.

This ADR decides the **conflict resolution semantics** the sync engine (#126)
applies when it replays a pulled event into local SQLite. The roadmap already
committed to the shape of the answer:

- **§2.5 "Conflict policy by entity class"** gives a per-entity-class table
  (LWW with audit, set-union with tombstones, per-field LWW with conflict note,
  conflict copies for bodies, append-only merges).
- **Decision #22** fixes the guiding principle: *"append-only events merge
  automatically; concurrent edits to the same note/card body create a conflict
  inbox item"* — knowledge data tolerates more auto-merge than finance, but
  **silent loss of user-authored prose is unacceptable**.

The problem is that a single global strategy is wrong. Last-writer-wins is right
for a document's read state but destroys a concurrently edited note. Set-union
is right for tags but nonsensical for a scalar title. Blind LWW on a review
card's FSRS scheduling state can corrupt due counts and reps. The resolution
strategy must therefore be **typed by entity — and by field within an entity**.

### Constraints inherited from prior ADRs

- **ADR-007 / ADR-001 (zero-I/O core):** conflict resolution is pure computation
  over two entity states plus their clocks. It lives in `pergamon-core` (or the
  Apache-2.0 client sync crate) as deterministic, table-driven merge functions
  with no I/O, so it is exhaustively unit-testable and reusable across CLI, iOS,
  and web. The server does no merging — it cannot read plaintext (ADR-022).
- **ADR-022 (envelope):** resolution consumes only what the envelope already
  carries (`clock`, prior `version`, `op`, per-field `fields`). This ADR adds no
  new wire fields; conflict copies and tombstones are ordinary entities/events.
- **ADR-006 (schema):** every entity has a stable UUID and `updated_at`;
  soft-delete tombstones are already part of the sync schema (`tombstones`).
- **ADR-005 (FSRS):** review scheduling state (`stability`, `difficulty`,
  `reps`, `lapses`, `state`, `due_at`) is derived by a pure scheduler from an
  append-only review history. That derivability is the key that lets us merge
  review state without corruption.

## Decision

### Ordering primitive: the hybrid logical clock, with a deterministic tiebreak

All comparisons use the ADR-022 **HLC**, not wall-clock time. For two versions
of the same entity/field, the winner is the one with the greater
`(wall_millis, counter)`; ties (genuine concurrency or equal stamps) break
**deterministically** on the larger `device_id`. Determinism matters more than
"who was really later": every device must reach the **same** merged state from
the same set of events regardless of pull order, so the tiebreak must be a total
order, not a coin flip. Wall-clock skew cannot silently pick a winner because
the HLC counter and `device_id` disambiguate equal or reordered wall times.

Two edits are **concurrent** (a *conflict*) when neither's clock causally
descends from the other — i.e. the writer's observed prior `version` does not
match the version its edit is being applied onto. Concurrency is what triggers
the per-entity policy below; a causal (fast-forward) update never conflicts and
always applies.

### Four merge strategies

Every entity field is assigned exactly one of four strategies:

| Strategy | Behavior on concurrency | Loses data? |
|---|---|---|
| **LWW** (last-writer-wins) | Keep the value with the greater HLC; discard the other. | Yes — by design, only for low-stakes scalars. |
| **Set-union + tombstone** | Union additions; a delete wins only over adds it causally dominates (observed-remove). Re-adding after a dominated delete resurrects. | No. |
| **Derived-merge** | Do not LWW the scalar. Recompute it from the union of an append-only history (review events / logs). | No. |
| **Conflict-copy** | Keep both bodies: retain the loser as a sibling entity and surface it in the conflict inbox. | No — never silently. |

The whole policy is **which strategy applies to which field of which entity**.

### Per-entity-type policy

#### Documents — per-field, mixed strategy

A document (`ContentItem`) mixes low-stakes scalars, user-authored prose, and
set-like edges. Merge is **per field**, using `field_patch` granularity so two
devices editing *different* fields never conflict at all:

| Field(s) | Strategy | Rationale |
|---|---|---|
| `status`, `read_at`, `later`/`reference` intent, starred/pinned flags | **LWW** | Cheap triage flags; the most recent decision wins. "`reading` → `archived`" on one device and a highlight added on another **both survive** because they touch different entities/fields. |
| `title` override, `author`, `excerpt` overrides | **LWW** | Scalar metadata; concurrent same-field edits are rare and low-value. |
| User-authored **inline body/notes on the document** (if any long-form prose) | **Conflict-copy** | Prose is never silently dropped. |
| Tag membership, collection membership | **Set-union + tombstone** | Adds from both devices merge; see below. |
| Extraction-derived fields (`content_text`, reading time, canonical URL) | **Derived / immutable** | Recomputed from the raw blob (immutable, dedup-by-hash per ADR-022), not user state — never a conflict; the newest extraction by clock wins. |

`updated_at` is set to the max of the merged inputs; it is bookkeeping, never a
conflict source, and is **never** used as the ordering primitive (that is the
HLC).

#### Read / triage state — LWW, no conflict surfaced

Read state, triage status, and intent are the highest-churn, lowest-stakes data.
They are always **LWW** and never produce a conflict inbox item: a stale
"unread → read" losing to a newer "read → unread" is expected, self-correcting,
and not worth a user's attention. An **audit trail** (the prior value + winning
clock) is retained for `feed_subscription`/settings-class flags so a surprising
flip is explainable, matching §2.5's "LWW with audit trail".

#### Tags and collections — set-union with observed-remove tombstones

Tag and collection **membership edges** (document↔tag, document↔collection,
collection parent link) are a set CRDT:

- Concurrent **adds** on different devices **both survive** (union). "Two devices
  add different tags to the same document" → the document ends with both tags.
- A **delete** is a tombstone that removes only the adds it **causally
  dominates** (observed-remove semantics). A concurrent add the delete never saw
  **wins** — the edge survives — so a rename-then-retag race doesn't silently
  drop a tag.
- **Re-adding** an edge after a dominated delete **resurrects** it (a new add
  with a later clock dominates the tombstone).

The tag/collection **entities themselves** (the `Tag.name`, the `Collection`
`name` / `parent_id` / `sort_order` / smart-`filter_query`) are ordinary records
merged **per-field LWW**; two devices renaming the same collection concurrently
is LWW on `name`, not a conflict copy, because a collection name is not
authored prose. Deleting a non-empty collection is a tombstone on the collection
whose member edges tombstone independently — surviving concurrent adds keep the
documents, they just lose that one collection edge.

#### Annotations (highlights and notes) — append-add, conflict-copy the body

Annotations are user-authored and sometimes edited concurrently:

- **Creating** a highlight or note is an append — different annotations from
  different devices **always merge**. "Two devices add separate highlights" →
  both exist. A highlight's identity is its `entity_id`; its anchor
  (`position_start`/`position_end`, `quote_text`) is treated as **immutable**
  provenance — a re-anchor is a new annotation, never an in-place mutation, so
  anchors never conflict.
- **Editing the same annotation body** concurrently (the `Note.body`, or a
  highlight's attached `note`/`color`) is a **conflict-copy**: the higher-clock
  edit becomes the live body; the losing edit is preserved as a **sibling note
  linked to the same content item** and raised in the conflict inbox for the
  user to reconcile or dismiss. We never LWW authored prose.
- **Deleting** an annotation is a tombstone; a concurrent body edit the delete
  did not observe **resurrects** the annotation as a conflict copy rather than
  letting a delete silently erase an edit the deleter never saw.

#### Review / FSRS state — append-only history, derived scheduling (never LWW the schedule)

This is the case where naïve LWW is actively harmful: two devices that both
review the same card would, under LWW, keep only one review and **corrupt
`reps`, `lapses`, and `due_at`**. Instead we split the card into its
**append-only history** and its **derived schedule**:

- **`ReviewLog` (review events) are append-only and always auto-merge.** Each is
  an immutable record of one rating at one time, keyed by its own `entity_id`.
  The union of both devices' logs is the true history; duplicates are
  idempotent by id.
- **`ReviewCard` scheduling state (`state`, `stability`, `difficulty`,
  `due_at`, `review_count`/`reps`, `lapse_count`/`lapses`, `scheduled_days`) is
  NOT stored-value-merged. It is *recomputed* by the deterministic FSRS
  scheduler (ADR-005) by folding the **time-ordered union of all review logs**
  for that card.** Because FSRS is a pure, order-dependent function of the log,
  every device that has the same set of logs derives the **same** card state —
  no counts double, none are lost, and `due_at` is consistent. This is the
  **derived-merge** strategy, and it is why review state can merge without a
  conflict inbox entry.
- Concurrent reviews at nearly the same instant are ordered by HLC; the fold is
  deterministic under that order, so the merged `due_at`/`reps` are reproducible
  regardless of which device pulled first.
- **Enable/disable** of review on a highlight (card existence) is **LWW** on the
  card's lifecycle flag; the append-only log survives a disable so a later
  re-enable rebuilds correct state rather than resetting it.

#### Feed subscriptions, settings, and other mutable config — LWW with audit

Feed subscriptions and app settings are mutable config with no authored prose:
**per-field LWW with an audit trail** (§2.5). Health/fetch bookkeeping
(`etag`, `last_fetched_at`, health counters) is device-local sync metadata, not
synced content, so it never participates in cross-device merge.

#### Blobs and immutable revisions — dedup by hash, no logical conflict

Per ADR-022, blobs (HTML snapshots, PDFs, extracted-text snapshots) are
content-addressed by ciphertext hash and immutable. Identical content
deduplicates; a materially different re-snapshot is a **new** blob referenced by
a new event. There is **no logical conflict** — divergent revisions coexist as
distinct immutable blobs, and the document's *reference* to "current" revision
is an LWW scalar field on the document.

### Summary table

| Entity / field class | Sync shape | Conflict strategy | User-surfaced? |
|---|---|---|---|
| Document read/triage state, intent, starred | mutable scalar | **LWW** | No |
| Document title/author/excerpt overrides | mutable scalar | **per-field LWW** | No |
| Document authored body (if any) | mutable prose | **Conflict-copy** | Yes |
| Document extraction-derived fields | derived | recompute; newest-clock | No |
| Tag / collection membership edges | set | **Set-union + observed-remove tombstone** | No |
| Tag / collection entity (name, parent, order, query) | mutable scalar | **per-field LWW** | No |
| Annotation creation (highlights, notes) | append | **auto-merge** | No |
| Annotation body / note text / color | mutable prose | **Conflict-copy** (+ resurrect on delete/edit race) | Yes |
| Review events (`ReviewLog`) | append-only | **auto-merge** | No |
| Review card schedule (FSRS state) | derived | **derived-merge from log union** | No |
| Review enable/disable | lifecycle flag | **LWW** | No |
| Feed subscriptions / settings | mutable config | **LWW + audit trail** | No |
| Blobs / immutable revisions | immutable | **dedup by hash; no conflict** | No |

### The conflict inbox

A conflict is only ever surfaced for the **Conflict-copy** cases (authored
document bodies and annotation/note bodies). Surfacing means:

- The **live** value is the HLC winner, so the app is never blocked and reading
  continues normally.
- The **losing** value is retained as a first-class sibling entity (a conflict
  copy) linked to the same parent, tagged as a conflict, and listed in a
  **conflict inbox** view (a TUI/CLI/app surface; not a server concept — the
  server never learns a conflict happened, ADR-022).
- The user resolves by keeping one, keeping both, or merging manually; resolution
  is itself an ordinary edit that produces a new outbox event. Dismissing a
  conflict copy tombstones it.

Everything else — read state, tags, collections, new annotations, review
history and schedule — **auto-merges with zero user interaction**. This is the
hybrid the roadmap asks for: practical, not cavalier.

## Consequences

### Positive

- **No silent loss of authored content.** The only lossy strategy (LWW) is
  confined to low-stakes scalars (triage flags, names, config). Every piece of
  user-authored prose is either auto-merged (distinct annotations) or preserved
  as a conflict copy — never overwritten silently.
- **Review counts can't corrupt.** Deriving FSRS state from the append-only log
  union (rather than merging stored `reps`/`due_at`) makes concurrent reviews
  correct and deterministic across devices, honoring ADR-005's "schedule is a
  pure function of history".
- **Most edits never bother the user.** Tags, collections, read state, and new
  highlights merge automatically; the conflict inbox stays small and meaningful.
- **Deterministic and testable.** Table-driven, per-field strategies over HLC
  inputs are pure functions in `pergamon-core` — exhaustively unit-testable and
  identical across CLI/iOS/web (ADR-007/ADR-001).
- **No new wire surface.** Resolution consumes only ADR-022's existing
  `clock`/`version`/`fields`; conflict copies and tombstones are ordinary
  entities and events, so the AGPL server stays a blind log (ADR-022/ADR-008).
- **Convergent.** The deterministic HLC tiebreak plus observed-remove set
  semantics guarantee all devices reach the same state from the same event set,
  regardless of pull order.

### Negative

- **Per-field granularity adds machinery.** Documents must diff and merge at the
  field level and emit `field_patch` events, which is more complex than
  whole-entity replace and requires the client to track per-field clocks.
- **Conflict copies can accumulate.** A user who ignores the conflict inbox
  collects sibling copies; the app must make review and bulk-dismiss easy, or
  clutter grows.
- **Observed-remove tombstones must be retained.** Set-union correctness needs
  delete tombstones kept long enough to dominate laggard adds, adding retention
  bookkeeping (bounded by the same device-cursor GC ADR-022 already requires).
- **Derived review state must replay history.** Recomputing FSRS state from the
  log union costs more than reading a stored scalar, and requires the full log
  (or a snapshot checkpoint) to be present before the card's schedule is
  authoritative; a device syncing metadata-first may show a briefly stale
  `due_at` until logs arrive.
- **HLC correctness is load-bearing.** Poorly maintained clocks (counter not
  advanced on equal wall time) would make the tiebreak non-total; the client
  must implement HLC updates rigorously.

## Rejected Alternatives

- **A single global strategy (LWW everywhere).** Rejected: LWW on a note body,
  an annotation, or FSRS `reps` silently destroys user work and corrupts review
  counts. The stakes differ by entity, so the strategy must too.
- **CRDT everything (e.g. RGA/text CRDT for every body).** Rejected as
  over-engineering for a local-first, mostly single-user-across-devices tool.
  Character-level text merge of prose adds large complexity and storage for a
  rare event; a conflict copy preserves both intents at a fraction of the cost
  and lets the human — who has the semantic context — decide. Set-union CRDT
  semantics *are* used where they're cheap and clearly right (tags/collections).
- **Wall-clock timestamps for ordering.** Rejected: cross-device skew makes
  wall-clock winners arbitrary and non-convergent. The ADR-022 HLC with a
  deterministic `device_id` tiebreak gives a total, causally-aware order.
- **LWW-merging stored FSRS card state.** Rejected: it double-counts or drops
  reviews and corrupts `due_at`/`lapses`. Treating logs as the append-only truth
  and *deriving* the schedule is the only way to merge review state losslessly.
- **Surfacing every concurrent edit to the user.** Rejected: prompting on a
  concurrent read-state or tag change would make sync exhausting and train users
  to dismiss conflicts blindly — devaluing the inbox for the cases that matter.
  Only authored-prose bodies are surfaced.
- **Server-side conflict resolution.** Rejected: the server holds only
  ciphertext (ADR-022) and cannot read entity types or fields, so it *cannot*
  merge. Resolution must be client-side, which also keeps it in the zero-I/O
  testable core (ADR-007) and out of the AGPL server (ADR-008).
- **Auto-merging annotation/note bodies by concatenation.** Rejected: blind
  concatenation produces garbled text and hides that a real divergence happened.
  A conflict copy keeps both intact and legible.
