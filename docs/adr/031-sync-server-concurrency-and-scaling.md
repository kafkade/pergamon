# ADR-031: Sync Server Concurrency and Scaling

**Status**: Accepted
**Date**: 2026-07-27
**Deciders**: kafkade

## Context

`pergamon-sync-server` is the AGPL-3.0 blind relay (ADR-022, ADR-026). Until
WP-3e (#201) its entire storage layer was:

```rust
pub struct AppState {
    pub store: Arc<Mutex<SyncStore>>,   // one process-wide lock
}

pub struct SyncStore {
    conn: Connection,                   // exactly one SQLite connection
}
```

Two consequences followed, and both are load-bearing for the managed-hosting
work this ADR closes out:

1. **Everything serialized.** Every request — read or write, any tenant — took
   the same `std::sync::Mutex`. Two tenants pulling event pages could not
   overlap at all, however many cores the host had.
2. **The database was not in WAL mode.** `SyncStore::open` set only
   `PRAGMA busy_timeout = 5000`, leaving SQLite's default rollback journal, under
   which readers block the writer and the writer blocks readers. So even removing
   the mutex would have bought very little.

WP-3c (#197) added tenant isolation and WP-3d (#198) added per-tenant storage
quotas, both of which assume many tenants share one process. WP-4 (#195) added
per-IP rate limiting and a global concurrency limit with load-shedding, and
explicitly deferred **per-tenant fairness** to this issue, noting that its
controls bound aggregate load but give no per-tenant guarantee because everything
contended on the single store mutex anyway.

This ADR records how the relay scales, and — just as importantly — where it
stops scaling.

## Decision

### 1. WAL, with an explicit `synchronous` choice

The store opens in WAL mode. Pragmas and rationale:

| Pragma | Value | Rationale |
|---|---|---|
| `journal_mode` | `WAL` | Readers no longer block the writer, and the writer no longer blocks readers. This is the precondition that makes a reader pool worth anything. |
| `synchronous` | `NORMAL` | The standard WAL pairing. Durable across a process crash; at worst loses the last transaction on power loss. Defensible here specifically because ADR-026 already establishes that **clients are the source of truth** and a lost server database is recoverable by re-syncing from a device. |
| `busy_timeout` | `5000` (unchanged) | Cross-process backstop. Within the process, writes cannot collide (see below), so this now only matters for an external tool holding the database. |
| `query_only` | `ON`, readers only | Defence in depth for the read/write split: a method mis-classified as a read fails loudly instead of quietly writing through a connection the design assumes is read-only. |
| `wal_autocheckpoint` | default (1000 pages) | No reason to deviate; recorded so the default is a decision rather than an oversight. |

WAL creates `-wal` and `-shm` sidecar files. **This contradicts ADR-026 as
originally written**, which stated that "under normal operation there are no WAL
sidecar files" and built its backup guidance on that. ADR-026 has been amended
accordingly; see [§Backup](#backup-consequences) below.

### 2. One explicit writer connection, plus a bounded reader pool

```rust
pub struct SyncStore {
    writer:  Mutex<Connection>,   // one writer — SQLite's real ceiling, made structural
    readers: Option<ConnectionPool>,
    quota:   QuotaConfig,
}
```

SQLite permits exactly **one** writer at a time, even in WAL mode. Rather than
hide that behind a uniform read/write pool whose connections would race and
collide on `SQLITE_BUSY`, the writer is a single connection behind a mutex and
only readers are pooled. This has three benefits over a uniform pool:

- Writes serialize *in-process*, so write/write `SQLITE_BUSY` cannot occur at all
  within the server. The remaining `SQLITE_BUSY` sources are external processes.
- The ceiling is visible in the type, not folklore.
- Check-then-write sequences (`blob_put`'s quota check, `push_events`'
  dedup-then-append) remain atomic with respect to other writes, exactly as they
  were under the old global mutex. No WP-3d quota semantics changed.

Every store method now takes `&self` and acquires precisely what it needs.

### 3. A hand-rolled pool, not `r2d2`

The pool is ~150 lines: `Mutex<Vec<Connection>>` + `Condvar`, a checkout bounded
by a timeout, and an RAII guard that returns the connection on drop (including
during unwind). No new dependencies.

`r2d2` 0.8.10 (MIT OR Apache-2.0) + `r2d2_sqlite` 0.35.0 (MIT) was the obvious
alternative — both AGPL-compatible, and `r2d2_sqlite 0.35` requires exactly
`rusqlite ^0.40`, our pin, so it would have introduced no duplicate major
versions. It was rejected because its value-add — connection health checks, idle
reaping, and a background `scheduled-thread-pool` thread — exists to recycle dead
*network* connections. A SQLite connection is a local file handle that does not
die, so those features buy nothing while adding dependency surface to an AGPL
crate that we deliberately keep small. Using std primitives also avoided enabling
`tokio`'s `sync` feature, which this crate does not otherwise need.

### 4. Blocking work runs on `spawn_blocking`

Handlers are `async`; SQLite calls are blocking. Running them inline was
acceptable when one mutex meant one in-flight operation, but with a reader pool
of N it would let N slow reads occupy every Tokio worker thread, letting a single
heavy tenant starve the runtime — precisely the failure this work exists to
prevent.

All store access therefore goes through two `AppState` helpers,
`with_store` and `with_tenant_store`, which move the closure onto
`tokio::task::spawn_blocking`. This keeps the change to a single seam rather than
16 bespoke handler rewrites, and gives a natural place to map a panicked blocking
task to a 500.

One closure is one connection checkout. Where a handler previously relied on the
global mutex for atomicity across two statements, that is now requested
explicitly: `GET /v1/events` uses `SyncStore::pull_page`, which reads the page and
the high-water mark inside one deferred read transaction — a true MVCC snapshot
under WAL. Without this, a concurrent push could appear in the high-water mark
but not the page.

### 5. Per-tenant fairness: a concurrency cap, honestly scoped

A pool creates the gap WP-4 predicted: one heavy tenant can hold every pooled
connection. `TenantLimiter` caps how many store operations a single `account_id`
may have in flight. Over-cap callers wait up to the checkout timeout and are then
shed with `503`, matching the semantics of WP-4's global load-shed layer rather
than inventing a new status for the same condition.

The default cap is `read_pool_size - 1`. **What that guarantees:** no single
tenant can occupy the last reader connection, so another tenant always finds
capacity. **What it does not:** it is not proportional fairness — with many
tenants above the cap the pool remains first-come-first-served among them — it
does not weight tenants by plan or cost, and it is not a rate limit (that is
WP-4's job). A single-tenant self-host is effectively unaffected, which is why
this can ship on by default.

The tenant map is keyed on `account_id`, which in blind mode is unauthenticated,
attacker-supplied input. Entries are removed the moment a tenant's in-flight
count returns to zero, so the map is bounded by *concurrently active* tenants and
never by the number of distinct identifiers ever seen.

### 6. Operator configuration

Mirrors the existing WP-4 `AbuseConfig` / WP-3d `QuotaConfig` flag+env pattern.

| Flag | Environment variable | Default |
|---|---|---|
| `--read-pool-size` | `PERGAMON_READ_POOL_SIZE` | `8` |
| `--store-checkout-timeout-ms` | `PERGAMON_STORE_CHECKOUT_TIMEOUT_MS` | `5000` |
| `--max-tenant-concurrency` | `PERGAMON_MAX_TENANT_CONCURRENCY` | `0` = derive (`pool - 1`) |
| `--no-tenant-concurrency-limit` | `PERGAMON_NO_TENANT_CONCURRENCY_LIMIT` | `false` |

## Measured effect

From `crates/pergamon-sync-server/tests/load_concurrency.rs`, run with
`cargo test -p pergamon-sync-server --release --test load_concurrency -- --ignored --nocapture`.
Workload: 8 concurrent tenants × 40 event-page pulls (500 events per page, 2000
events seeded per tenant) — the hot read path of the ADR-022 protocol. "Before"
is the same code with `read_pool_size = 1`, which reproduces the pre-WP-3e
topology exactly, so both numbers come from one machine under identical
conditions.

On a 14-core host, release build:

| Path | Before (1 connection) | After (WAL + 8-connection pool) | Ratio |
|---|---|---|---|
| Read, store level | 76.6 ms — 4,175 pulls/s | 22.7 ms — 14,096 pulls/s | **3.38×** |
| Read, HTTP level | 75.8 ms — 4,224 req/s | 39.2 ms — 8,164 req/s | **1.93×** |
| Write, HTTP level | 18.9 ms — 16,915 req/s | 18.7 ms — 17,100 req/s | 1.01× |

Three things are worth reading off that table honestly:

- The **store-level** ratio is the one that answers "concurrent tenants no longer
  serialize behind one connection".
- The **HTTP-level** ratio is lower by construction: per-request HTTP, JSON and
  base64 work is untouched by this change and dilutes the store's share of each
  request. It is still the number an operator experiences.
- The **write** ratio is ~1.0, and that is the expected, correct result. Pooling
  is a read-path win. Anyone reporting a write speed-up from this change has
  measured something else.

Large-blob downloads were tried first as the benchmark workload and rejected: a
multi-megabyte `blob_get` is dominated by one memory copy, so it saturates memory
bandwidth rather than the connection and under-reports the pool's effect for
reasons unrelated to locking.

The always-on, non-flaky counterpart lives in `tests/e2e_concurrency.rs`. Its
headline test holds N reader connections across a timeout-aware rendezvous, which
a serialized store could never satisfy — a hard regression test with no
wall-clock threshold.

## The scaling ladder from here

In rough order of cost, and none of it implemented:

1. **Vertical.** Raise `--read-pool-size` and give the host more cores and page
   cache. Cheapest, and sufficient for a long way.
2. **Reduce per-request work.** The pool makes reads concurrent; it does not make
   them cheaper. Response caching and cursor-friendly indexes would.
3. **Shard by account.** The data model is already per-`account_id` with no
   cross-tenant queries anywhere, so partitioning tenants across N databases (or
   N processes) is the natural next step and would multiply the **write**
   ceiling, which pooling cannot.
4. **Read replicas.** WAL supports readers on a replicated file, but the relay's
   read-your-writes expectations after a push would need care.
5. **Leave SQLite.** Postgres removes the single-writer ceiling entirely. This is
   a real option for managed hosting at scale, and deliberately not taken now:
   SQLite keeps the self-host story a single binary plus a single file (ADR-006,
   ADR-026), which is a core product property.

### Backup consequences

WAL means `pergamon-sync.db` alone is **no longer a complete backup**. Recorded
in ADR-026 and `docs/sync-server.md`; the short version:

- The documented procedure (stop the container → `tar` the whole `/data` → start)
  remains correct, because a clean shutdown checkpoints the WAL and removes the
  sidecars. The documented restore's `rm -f /data/*` also clears stale sidecars.
- A **hot** copy of only the main database file is now incomplete and may be
  unusable. Either stop the container, or snapshot all three files atomically.

## Consequences

### Positive

- Concurrent tenants no longer serialize: 3.38× more concurrent read throughput
  at the store level, 1.93× end to end, on a 14-core host.
- The single-writer ceiling is explicit in the code, in this ADR, and in the
  benchmark output, instead of being an unstated assumption.
- One heavy tenant can no longer monopolize the reader pool, closing the gap WP-4
  deliberately left open.
- A slow tenant can no longer starve the Tokio runtime, because store work runs
  on the blocking pool.
- No new dependencies in an AGPL crate.
- The public API is unchanged where it mattered: `SyncStore::open`,
  `open_in_memory`, `with_quota` and `AppState::new` all still work, so all nine
  existing e2e suites compile untouched.

### Negative

- WAL sidecar files invalidate part of ADR-026 as originally written and make
  naive hot backups incomplete. Mitigated by documentation, but it is a genuine
  new footgun for operators who wrote their own backup script against the old
  text.
- `synchronous = NORMAL` accepts the loss of the last transaction on power loss.
  Acceptable only because clients are the source of truth.
- A hand-rolled pool is code we own and must keep correct. Mitigated by unit
  tests covering concurrent checkout, timeout, wake-on-return, and return during
  panic unwind.
- More live connections means more file handles and more page cache per process.
- `spawn_blocking` adds a task hop to every store call — irrelevant next to a
  SQLite query, but not zero.
- The default per-tenant cap is a behavior change for a multi-tenant deployment
  that previously had no per-tenant limit. Operators can disable it.

## Rejected Alternatives

- **`r2d2` + `r2d2_sqlite`.** See §3. Licensing and version compatibility were
  both fine; the features simply do not apply to local file handles.
- **A uniform read/write connection pool.** Rejected: SQLite still allows one
  writer, so this converts an in-process mutex into cross-connection
  `SQLITE_BUSY` contention — the same ceiling, discovered later and less
  clearly, plus it would have silently broken the check-then-write atomicity that
  WP-3d quota enforcement relies on.
- **Keeping blocking calls inline on Tokio workers.** Rejected: with a pool
  larger than the worker count, one heavy tenant could starve the runtime.
- **Pooling in-memory stores via `file:…?mode=memory&cache=shared`.** Rejected:
  shared-cache mode raises `SQLITE_LOCKED`, which `busy_timeout` does *not* cover
  without `sqlite3_unlock_notify`, so it would have made the test suite flaky in
  exchange for concurrency no test needs. `open_in_memory` instead degenerates to
  a single connection serving both reads and writes — byte-for-byte the previous
  behavior. Real concurrency is exercised against a file-backed store, which is
  what deployments use.
- **Pooling the OPAQUE auth store too.** Deferred, not forgotten. The auth plane
  (`src/auth/`) keeps its single mutexed connection: it is multitenant-only and
  its cost is dominated by OPAQUE and Argon2 CPU rather than SQLite, so pooling it
  would buy little. The more interesting follow-up there is that Argon2 runs on a
  Tokio worker thread and should move to `spawn_blocking` — a separate concern
  from this ADR.
- **A CI job for the benchmark.** Rejected: `required_status_checks` are managed
  by Terraform in `kafkade/github-infra`, so adding a job needs an
  infrastructure change, and a timing benchmark is a poor merge gate anyway. The
  benchmark is `#[ignore]`d and run by hand; the non-flaky concurrency assertions
  run on every build.
- **Switching to Postgres now.** Rejected for this iteration: it would break the
  single-binary, single-file self-host story that ADR-006 and ADR-026 are built
  on. Kept on the ladder above for managed hosting at scale.
