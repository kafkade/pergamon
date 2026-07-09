# ADR-025: Background Sync Scheduling and Offline Policy

**Status**: Accepted  
**Date**: 2026-07-04  
**Deciders**: kafkade

## Context

ADR-022 (#121) fixed the sync **wire contract**, ADR-023 (#122) fixed **conflict
resolution**, and ADR-024 (#125) fixed the **key hierarchy and device
lifecycle**. The resulting engine (`pergamon-sync`, #126/#127) exposes only
one-shot `push`, `pull`, and `sync` operations, driven manually through the CLI
`sync-remote` commands.

Issue #129 (epic #35) asks for **background sync behaviors on the web and iOS
clients**, with backoff, retry, and offline tolerance, so that "an item updated
on one device appears on another after sync without manual steps." That requires
deciding:

1. **Where the scheduling/backoff logic lives** so it is written and tested once
   rather than re-implemented per platform.
2. **What counts as offline/transient vs fatal**, so a disconnected device backs
   off and keeps trying instead of surfacing an error or exiting.
3. **How long-lived hosts (CLI daemon, web server) vs one-shot hosts (iOS
   background tasks) consume it.**
4. **How the AGPL web server unlocks account keys** without pulling
   Apache-licensed key handling into an AGPL crate, or vice versa.

## Decision

### 1. A pure scheduler core in `pergamon-sync`

The heart is two pure, clock-free, I/O-free types in `pergamon-sync::schedule`:

- **`BackoffPolicy`** — exponential backoff with equal jitter, capped. `ceiling
  = base × multiplier^retry`, clamped to `max`; `delay(retry, rand01)` draws
  uniformly from `[ceiling/2, ceiling]`. Randomness is injected (a `rand01`
  fraction), never sampled internally, so it is fully deterministic under test.
- **`SyncScheduler`** — a small state machine tracking consecutive failures.
  `next_delay(rand01)` returns the healthy `interval` when the last round
  succeeded, otherwise `backoff.delay(consecutive_failures - 1, rand01)`.
  `record_success` resets; `record_failure` deepens.

This lives in `pergamon-sync` (an already I/O-adjacent client crate), **not**
`pergamon-core`, which stays strictly zero-I/O per ADR-001. The policy itself is
pure, but keeping it beside the engine keeps the dependency direction clean.

### 2. Offline tolerance = retryable error classification

`SyncError::is_retryable()` classifies transport/network failures (and
not-found, which can be a transient relay state) as **retryable**; crypto,
serialization, and protocol errors are **fatal**. The background driver treats
retryable errors as "offline": it records a failure, backs off, and keeps
running. Fatal errors stop the loop (long-lived hosts) or surface as an error
(one-shot hosts).

### 3. Two host shapes over one core

- **Long-lived hosts** (CLI `sync-remote daemon`, web server worker) use
  `pergamon_sync::run_forever`: a blocking loop that runs a round, updates the
  scheduler, reports via an observer callback, then sleeps for the scheduled
  delay via an **injectable** `Sleeper`. The production `Sleeper` is an mpsc
  `ChannelSleeper` whose paired `SyncControl` lets callers `trigger()` an
  out-of-band round or `shutdown()` the loop; dropping every control handle also
  stops the loop. A round runs immediately on start (sync-on-launch), and the
  loop is driven by a deterministic `Jitter` PRNG (splitmix64) so tests can pin
  the sequence.
- **One-shot hosts** (iOS `BGTaskScheduler`) call
  `Library::background_refresh()` once per wake. It runs a single `sync`, and
  returns a `BackgroundRefreshResult { pushed, applied, offline,
  retry_after_seconds }`. `retry_after_seconds` is derived from the **same**
  `SyncScheduler` (healthy cadence on success, backoff delay when offline), so
  the OS scheduler gets a principled next-wake hint. The iOS refresh interval
  floor is 15 minutes (`BGAppRefreshTask`), so that is the healthy cadence.

### 4. Shared `pergamon-keystore` crate (Apache-2.0)

The encrypted-key-file / OS-keyring `DeviceKeyStore` moved out of
`pergamon-cli` into a new Apache-2.0 crate, `pergamon-keystore`, so the AGPL
`pergamon-server` can unlock account keys without a license conflict. The
`keyring` (OS keychain) backend is gated behind a default feature: the CLI
enables it; the server depends on the crate with `default-features = false` and
uses only the encrypted-file backend, unlocked from a key-file path plus a
passphrase supplied via `--sync-key-passphrase` / `PERGAMON_SYNC_KEY_PASSPHRASE`.
iOS does not use this crate at all: Swift holds the account root key in the iOS
keychain and passes its bytes to `configure_sync(...)`, keeping the FFI narrow.

### 5. Server worker uses a dedicated database connection

The web server's background worker opens its **own** `pergamon-storage`
`Database` connection to the same SQLite file rather than sharing the request
handlers' `Mutex<Database>`. SQLite runs in WAL mode with `busy_timeout`, so
concurrent writers on separate connections are safe, and a network-bound sync
round never holds the request-serving mutex. Server-side document mutations
(create/update/delete and web-UI triage/bulk actions) emit change-tracking so
edits made in the browser are pushed on the next round. A `POST
/admin/sync-remote/trigger` endpoint (behind the same admin auth) forces an
immediate round; the worker is stopped on graceful shutdown.

## Consequences

- The scheduling/backoff logic is written and unit-tested once; every platform
  wires a thin driver around it. Convergence is covered by an integration test
  that drives two databases through a shared transport with the scheduler and
  asserts an edit propagates with no manual push/pull.
- Background sync is **opt-in** on every platform: the server worker only starts
  when a key file + passphrase are configured, and iOS sync is inert until
  `configure_sync` is called. This preserves the local-first, no-surprise
  posture.
- The iOS `background_refresh` uses blocking `reqwest` on the FFI thread, which
  is heavier than an async client but matches ADR-019's synchronous-blocking FFI
  model. A future revision could move iOS onto an `HttpRelay`-style transport if
  the binary-size/latency cost proves material.
- Holding the iOS `Library` DB lock across a network round is acceptable for a
  single-user device (the call is already blocking by design) but would not be
  for the multi-connection server — hence the server's separate connection.

## Alternatives considered

- **Per-platform schedulers.** Rejected: three copies of backoff/jitter logic to
  keep correct and in sync.
- **Async scheduler in the engine.** Rejected for now: the engine is synchronous
  and blocking; an injectable `Sleeper` gives testability without an async
  runtime dependency in the core loop, and each host supplies its own
  concurrency (OS thread on the server, `BGTaskScheduler` on iOS).
- **Sharing the server's request-handler DB mutex.** Rejected: it would serialize
  every request behind a network-bound sync round.
