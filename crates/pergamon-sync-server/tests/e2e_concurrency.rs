// SPDX-License-Identifier: AGPL-3.0-only

//! Concurrency and scaling tests for the sync server (WP-3e, [#201]).
//!
//! These prove the acceptance criterion "concurrent tenants no longer serialize
//! behind one connection" **without any wall-clock threshold**, so they are safe
//! to run on a loaded CI box. The opt-in benchmark that prints before/after
//! numbers lives in `load_concurrency.rs`.
//!
//! The headline test is [`concurrent_reads_genuinely_overlap`]: it holds `N`
//! reader connections open simultaneously across a rendezvous barrier. Under the
//! pre-WP-3e design — one connection behind one process-wide mutex — the barrier
//! could never be reached, so a regression is a hard failure (guarded by a
//! timeout so it fails rather than hangs).
//!
//! [#201]: https://github.com/kafkade/pergamon/issues/201

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pergamon_sync_server::{
    AppState, FairnessConfig, PoolConfig, SyncStore, build_router, ct_hash,
};
use serde_json::{Value, json};
use tower::ServiceExt;

/// How long a rendezvous waits before declaring the test failed.
const RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(20);

/// A reusable N-party rendezvous **with a timeout**.
///
/// `std::sync::Barrier` would be the obvious choice, but it cannot time out: a
/// regression that stopped reads from overlapping would hang the test binary
/// forever (and, worse, hang Tokio's blocking-pool shutdown) instead of
/// failing. Every party here gets `false` back when the rendezvous is not met in
/// time, so a regression is a clean assertion failure.
struct Rendezvous {
    /// `(arrived, generation)`.
    inner: Mutex<(usize, u64)>,
    ready: Condvar,
    parties: usize,
}

impl Rendezvous {
    const fn new(parties: usize) -> Self {
        Self {
            inner: Mutex::new((0, 0)),
            ready: Condvar::new(),
            parties,
        }
    }

    /// Arrive and wait for every other party. Returns `false` on timeout.
    fn wait(&self, timeout: Duration) -> bool {
        let mut guard = self.inner.lock().unwrap();
        let generation = guard.1;
        guard.0 += 1;
        if guard.0 == self.parties {
            guard.0 = 0;
            guard.1 = generation.wrapping_add(1);
            drop(guard);
            self.ready.notify_all();
            return true;
        }
        let deadline = Instant::now() + timeout;
        while guard.1 == generation {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, timed_out) = self.ready.wait_timeout(guard, remaining).unwrap();
            guard = next;
            if timed_out.timed_out() && guard.1 == generation {
                return false;
            }
        }
        true
    }
}

/// A unique temp path for a test database, cleaned up on drop.
struct TempDb {
    path: std::path::PathBuf,
}

impl TempDb {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pergamon-sync-concurrency-{}.db",
            uuid::Uuid::new_v4()
        ));
        Self { path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let p = format!("{}{suffix}", self.path.display());
            let _ = std::fs::remove_file(p);
        }
    }
}

/// A file-backed store with an explicit reader-pool size.
fn file_store(tmp: &TempDb, pool_size: usize) -> SyncStore {
    SyncStore::open_with_pool(
        &tmp.path,
        PoolConfig {
            size: pool_size,
            checkout_timeout: Duration::from_secs(10),
        },
    )
    .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

fn json_req(method: &str, uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// A single-event push batch for `account`.
fn push_body(account: &str, change_id: &str, payload: &[u8]) -> Value {
    json!({
        "account_id": account,
        "events": [{
            "protocol_version": 1,
            "account_id": account,
            "device_id": "device-A",
            "change_id": change_id,
            "key_epoch": 1,
            "blob_refs": [],
            "ciphertext_b64": STANDARD.encode(payload),
        }]
    })
}

// ---------------------------------------------------------------------------
// The store runs in WAL mode
// ---------------------------------------------------------------------------

/// WAL is what lets readers proceed without blocking (or being blocked by) the
/// writer. Without it a reader pool would buy almost nothing.
#[test]
fn a_file_backed_store_runs_in_wal_mode() {
    let tmp = TempDb::new();
    let store = file_store(&tmp, 4);
    assert_eq!(store.journal_mode().unwrap().to_lowercase(), "wal");
    assert_eq!(store.read_pool_size(), 4);

    // The sidecars ADR-026 was amended for really do appear.
    store
        .blob_put("acct", &ct_hash(b"opaque"), b"opaque")
        .unwrap();
    let wal = std::path::PathBuf::from(format!("{}-wal", tmp.path.display()));
    assert!(wal.exists(), "WAL mode must produce a -wal sidecar");
}

/// An in-memory store deliberately degenerates to one connection: every
/// `:memory:` connection is its own empty database, so pooling it would silently
/// hand different callers different stores.
#[test]
fn an_in_memory_store_is_a_single_connection_and_stays_consistent() {
    let store = SyncStore::open_in_memory().unwrap();
    assert_eq!(store.read_pool_size(), 1);

    store.blob_put("acct", &ct_hash(b"x"), b"x").unwrap();
    // A read after a write must see it — the failure mode a naive `:memory:`
    // pool would introduce.
    assert_eq!(
        store.blob_get("acct", &ct_hash(b"x")).unwrap().unwrap(),
        b"x"
    );
    assert_eq!(store.account_usage("acct").unwrap().blob_count, 1);
}

// ---------------------------------------------------------------------------
// The headline proof: reads genuinely overlap
// ---------------------------------------------------------------------------

/// `N` concurrent readers hold a store connection open **at the same time**.
///
/// The rendezvous is only satisfied if all `N` reads are in flight
/// simultaneously. Under one mutexed connection, reader 1 would hold the lock
/// while waiting and readers 2..N would block on it, so the rendezvous could
/// never be met — making this a hard, timing-free regression test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reads_genuinely_overlap() {
    const N: usize = 6;
    let tmp = TempDb::new();
    let store = Arc::new(file_store(&tmp, N));
    store
        .blob_put("acct", &ct_hash(b"payload"), b"payload")
        .unwrap();

    // INVARIANT (read this before changing N or the pool size). The rendezvous
    // can only be met if all three of these hold:
    //   1. N <= the store's reader-pool size — asserted immediately below, so a
    //      future change to `file_store`'s default cannot silently break it;
    //   2. N <= Tokio's blocking-thread pool (default 512), and
    //   3. nothing else caps concurrency for these tasks. This test drives the
    //      `SyncStore` directly and never builds an `AppState`, so the
    //      per-tenant `TenantLimiter` is deliberately NOT in play. If this were
    //      rewritten to go through `AppState` with a shared account_id, the
    //      default cap of `pool_size - 1` would shed the N-th reader and the
    //      failure would look like "concurrency is broken" when it was actually
    //      fairness working correctly.
    // If an invariant is ever violated, `Rendezvous` times out and this fails
    // cleanly — it does not hang (which is why it is not a `std::sync::Barrier`).
    assert_eq!(
        store.read_pool_size(),
        N,
        "this test needs a reader per task; see the invariant above"
    );

    let rendezvous = Arc::new(Rendezvous::new(N));
    let tasks: Vec<_> = (0..N)
        .map(|_| {
            let store = Arc::clone(&store);
            let rendezvous = Arc::clone(&rendezvous);
            tokio::task::spawn_blocking(move || {
                // Hold a pooled read connection across the rendezvous. All `N`
                // must be held simultaneously for it to be met.
                store
                    .with_read_connection(|| rendezvous.wait(RENDEZVOUS_TIMEOUT))
                    .unwrap()
            })
        })
        .collect();

    let results = join_all(tasks).await;
    assert_eq!(results.len(), N);
    assert!(
        results.into_iter().all(|overlapped| overlapped),
        "the store must hand out {N} simultaneous readers; a serialized store cannot"
    );
}

/// Await a set of blocking tasks without pulling in a futures crate.
async fn join_all(tasks: Vec<tokio::task::JoinHandle<bool>>) -> Vec<bool> {
    let mut out = Vec::with_capacity(tasks.len());
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

/// The pool is genuinely bounded: with every connection held, another reader is
/// shed with a retryable error rather than blocking forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_saturated_pool_sheds_rather_than_blocking_forever() {
    let tmp = TempDb::new();
    let store = Arc::new(
        SyncStore::open_with_pool(
            &tmp.path,
            PoolConfig {
                size: 2,
                checkout_timeout: Duration::from_millis(100),
            },
        )
        .unwrap(),
    );

    // Two rendezvous: the first proves both connections are actually held
    // *before* the probe runs (otherwise the probe races the holders), the
    // second releases them.
    let acquired = Arc::new(Rendezvous::new(3));
    let release = Arc::new(Rendezvous::new(3));
    let holders: Vec<_> = (0..2)
        .map(|_| {
            let store = Arc::clone(&store);
            let acquired = Arc::clone(&acquired);
            let release = Arc::clone(&release);
            tokio::task::spawn_blocking(move || {
                store
                    .with_read_connection(|| {
                        acquired.wait(RENDEZVOUS_TIMEOUT) && release.wait(RENDEZVOUS_TIMEOUT)
                    })
                    .unwrap()
            })
        })
        .collect();

    let both_held = acquired.wait(RENDEZVOUS_TIMEOUT);
    let store_for_probe = Arc::clone(&store);
    let probe = tokio::task::spawn_blocking(move || store_for_probe.blob_get("acct", "deadbeef"))
        .await
        .unwrap();
    // Always release before asserting, so a failure is a failure and not a hang.
    release.wait(RENDEZVOUS_TIMEOUT);
    let held_results = join_all(holders).await;

    assert!(both_held, "both pooled connections should have been held");
    assert!(held_results.into_iter().all(|ok| ok));
    assert!(
        probe.is_err(),
        "a saturated pool must time out, not block forever"
    );
    // Once the holders release, reads work again.
    assert!(store.blob_get("acct", "deadbeef").unwrap().is_none());
}

/// Reads must succeed *while a write is in flight* — the property WAL adds on
/// top of the pool. With the default rollback journal a writer's exclusive lock
/// would block these reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_proceed_while_a_write_is_in_flight() {
    let tmp = TempDb::new();
    let store = Arc::new(file_store(&tmp, 4));
    store.blob_put("acct", &ct_hash(b"seed"), b"seed").unwrap();

    // A chunky write: 400 blobs, each its own transaction.
    let writer = {
        let store = Arc::clone(&store);
        tokio::task::spawn_blocking(move || {
            for i in 0..400_u32 {
                let bytes = format!("blob-{i:08}").into_bytes();
                store
                    .blob_put("writer-tenant", &ct_hash(&bytes), &bytes)
                    .unwrap();
            }
        })
    };

    // Concurrently, hammer reads for a different tenant.
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                for _ in 0..100 {
                    assert!(store.blob_get("acct", &ct_hash(b"seed")).unwrap().is_some());
                }
                true
            })
        })
        .collect();

    // A generous ceiling on the whole interleaved run: it exists so a regression
    // that reintroduced reader/writer blocking fails instead of hanging CI.
    #[allow(clippy::duration_suboptimal_units)]
    let ceiling = Duration::from_secs(60);
    tokio::time::timeout(ceiling, async {
        for reader in readers {
            assert!(reader.await.unwrap());
        }
        writer.await.unwrap();
    })
    .await
    .expect("reads must not be blocked out by a concurrent writer");

    assert_eq!(
        store.account_usage("writer-tenant").unwrap().blob_count,
        400
    );
}

// ---------------------------------------------------------------------------
// Correctness under concurrency
// ---------------------------------------------------------------------------

/// Concurrent pushes from many tenants through the real router must stay
/// correct: every event lands exactly once, each tenant's `server_seq` is a
/// contiguous run from 1, and no tenant sees another's data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_pushes_across_tenants_stay_correct_and_isolated() {
    const TENANTS: usize = 6;
    const EVENTS_PER_TENANT: usize = 15;

    let tmp = TempDb::new();
    let app = build_router(AppState::new(file_store(&tmp, 8)));

    let tasks: Vec<_> = (0..TENANTS)
        .map(|t| {
            let app = app.clone();
            tokio::spawn(async move {
                let account = format!("tenant-{t}");
                for e in 0..EVENTS_PER_TENANT {
                    let body = push_body(
                        &account,
                        &format!("change-{e}"),
                        format!("ciphertext-{t}-{e}").as_bytes(),
                    );
                    let (status, _) = send(&app, json_req("POST", "/v1/events", &body)).await;
                    assert_eq!(status, StatusCode::OK, "push failed for {account}");
                }
            })
        })
        .collect();

    for task in tasks {
        task.await.unwrap();
    }

    for t in 0..TENANTS {
        let account = format!("tenant-{t}");
        let (status, body) = send(
            &app,
            Request::builder()
                .uri(format!("/v1/events?account_id={account}&after=0"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let resp: Value = serde_json::from_slice(&body).unwrap();
        let events = resp["events"].as_array().unwrap();

        assert_eq!(
            events.len(),
            EVENTS_PER_TENANT,
            "{account} lost or duplicated events under concurrency"
        );
        assert_eq!(resp["high_water_seq"], EVENTS_PER_TENANT as u64);

        // Sequences are a contiguous 1..=N run, with no gaps or repeats.
        let seqs: BTreeSet<u64> = events
            .iter()
            .map(|e| e["server_seq"].as_u64().unwrap())
            .collect();
        assert_eq!(
            seqs.len(),
            EVENTS_PER_TENANT,
            "{account} has duplicate seqs"
        );
        assert_eq!(
            seqs,
            (1..=EVENTS_PER_TENANT as u64).collect::<BTreeSet<_>>(),
            "{account} sequences are not contiguous from 1"
        );

        // Tenant isolation: every event belongs to this account only.
        for event in events {
            assert_eq!(event["account_id"], account);
            let ct = STANDARD
                .decode(event["ciphertext_b64"].as_str().unwrap())
                .unwrap();
            assert!(
                String::from_utf8(ct)
                    .unwrap()
                    .starts_with(&format!("ciphertext-{t}-")),
                "{account} received another tenant's ciphertext"
            );
        }
    }
}

/// Concurrent blob uploads and downloads across tenants stay content-addressed
/// and isolated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_blob_traffic_stays_isolated() {
    const TENANTS: usize = 5;
    let tmp = TempDb::new();
    let app = build_router(AppState::new(file_store(&tmp, 8)));

    let tasks: Vec<_> = (0..TENANTS)
        .map(|t| {
            let app = app.clone();
            tokio::spawn(async move {
                let account = format!("blob-tenant-{t}");
                let payload = format!("opaque-ciphertext-{t}").into_bytes();
                let hash = ct_hash(&payload);
                let (status, _) = send(
                    &app,
                    Request::builder()
                        .method("PUT")
                        .uri(format!("/v1/blobs/{account}/{hash}"))
                        .body(Body::from(payload.clone()))
                        .unwrap(),
                )
                .await;
                assert_eq!(status, StatusCode::CREATED);

                // The uploader can read it back.
                let (status, body) = send(
                    &app,
                    Request::builder()
                        .uri(format!("/v1/blobs/{account}/{hash}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
                assert_eq!(status, StatusCode::OK);
                assert_eq!(body, payload);

                // A different tenant does not hold it: content addressing is
                // per-account, and concurrency must not leak across tenants.
                let other = format!("blob-tenant-{}", (t + 1) % TENANTS);
                let (status, _) = send(
                    &app,
                    Request::builder()
                        .uri(format!("/v1/blobs/{other}/{hash}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
                assert_eq!(status, StatusCode::NOT_FOUND);
            })
        })
        .collect();

    for task in tasks {
        task.await.unwrap();
    }
}

/// `account_usage` must stay exact under concurrent writes, and must never
/// report a combination that never existed.
///
/// The writer strictly alternates `blob_put(i)` then `push_events(i)`, so real
/// history always satisfies `blob_count >= event_count`; a reading with
/// `event_count > blob_count` would be impossible.
///
/// **On what this does and does not prove.** The single-snapshot property of
/// `account_usage` is guaranteed *structurally* — the two aggregates run inside
/// one deferred read transaction, exactly like `pull_page` — not by this test.
/// The window between the two statements is sub-microsecond, so a torn read is
/// not reliably reproducible without a fault-injection hook in production code,
/// and this test was verified to pass with and without the transaction. It is
/// therefore an accounting-correctness test, not a proof of atomicity; it is
/// kept because "usage stays exact while writes land concurrently" is worth
/// pinning down on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_usage_stays_exact_under_concurrent_writes() {
    const ROUNDS: usize = 400;
    let tmp = TempDb::new();
    let store = Arc::new(file_store(&tmp, 4));
    let account = "acct-usage";

    let writer = {
        let store = Arc::clone(&store);
        tokio::task::spawn_blocking(move || {
            for i in 0..ROUNDS {
                let payload = format!("{i:016}").into_bytes();
                store
                    .blob_put(account, &ct_hash(&payload), &payload)
                    .unwrap();
                store
                    .push_events(
                        account,
                        &[pergamon_sync_server::store::EventRecord {
                            protocol_version: 1,
                            account_id: account.to_owned(),
                            device_id: "device-A".to_owned(),
                            change_id: format!("change-{i}"),
                            entity_ref: None,
                            key_epoch: 1,
                            blob_refs: Vec::new(),
                            ciphertext: payload,
                            signature: Vec::new(),
                        }],
                    )
                    .unwrap();
            }
        })
    };

    let readers: Vec<_> = (0..3)
        .map(|_| {
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                let mut impossible = 0_usize;
                for _ in 0..(ROUNDS * 3) {
                    let usage = store.account_usage(account).unwrap();
                    if usage.event_count > usage.blob_count {
                        impossible += 1;
                    }
                    // Every blob and every event is exactly 16 bytes, so the
                    // per-table aggregates must always agree internally.
                    assert_eq!(usage.blob_bytes, usage.blob_count * 16);
                    assert_eq!(usage.event_bytes, usage.event_count * 16);
                }
                impossible
            })
        })
        .collect();

    writer.await.unwrap();
    let mut impossible = 0;
    for reader in readers {
        impossible += reader.await.unwrap();
    }
    assert_eq!(
        impossible, 0,
        "saw {impossible} readings with event_count > blob_count, which the \
         writer's blob-then-event ordering makes impossible"
    );

    // The final accounting is exact: nothing lost or double-counted.
    let usage = store.account_usage(account).unwrap();
    assert_eq!(usage.blob_count, ROUNDS as u64);
    assert_eq!(usage.event_count, ROUNDS as u64);
    assert_eq!(usage.total_objects(), (ROUNDS as u64) * 2);
}

/// A saturated pool must degrade into a **retryable `503`** at the HTTP layer,
/// never a `500` and never a hang. This is the guarantee that makes it safe for
/// `with_store` to park a blocking-pool thread on the checkout `Condvar`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_saturation_surfaces_as_a_retryable_503_not_a_500() {
    let tmp = TempDb::new();
    let store = SyncStore::open_with_pool(
        &tmp.path,
        PoolConfig {
            size: 1,
            checkout_timeout: Duration::from_millis(50),
        },
    )
    .unwrap();
    // Fairness disabled, so the 503 provably comes from pool saturation rather
    // than from the per-tenant cap (both map to 503, so this isolates the cause).
    let state = AppState::with_fairness(store, FairnessConfig::disabled());
    let app = build_router(state.clone());

    let acquired = Arc::new(Rendezvous::new(2));
    let release = Arc::new(Rendezvous::new(2));
    let holder = {
        let store = Arc::clone(&state.store);
        let acquired = Arc::clone(&acquired);
        let release = Arc::clone(&release);
        tokio::task::spawn_blocking(move || {
            store
                .with_read_connection(|| {
                    acquired.wait(RENDEZVOUS_TIMEOUT) && release.wait(RENDEZVOUS_TIMEOUT)
                })
                .unwrap()
        })
    };

    let held = acquired.wait(RENDEZVOUS_TIMEOUT);
    let (status, body) = send(
        &app,
        Request::builder()
            .uri("/v1/events?account_id=acct&after=0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // Release before asserting, so a failure is a failure and not a hang.
    release.wait(RENDEZVOUS_TIMEOUT);
    let holder_ok = holder.await.unwrap();

    assert!(
        held && holder_ok,
        "the only connection should have been held"
    );
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "pool saturation must be a retryable 503, not a 500"
    );
    let err: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err["code"], "UNAVAILABLE");

    // And the server recovers once the connection is back.
    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/v1/events?account_id=acct&after=0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Per-tenant fairness
// ---------------------------------------------------------------------------

/// One tenant at its concurrency cap is shed with `503`, while a different
/// tenant is served normally. This is the guarantee WP-4 (#195) deferred here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_capped_tenant_is_shed_while_another_tenant_is_served() {
    let tmp = TempDb::new();
    // The cap is pinned explicitly rather than derived from the pool: this test
    // asserts a *shed*, so it must not silently change meaning if the default
    // `pool_size - 1` policy is ever retuned. The short wait keeps the over-cap
    // request shed promptly instead of queued.
    let state = AppState::with_fairness(
        file_store(&tmp, 4),
        FairnessConfig {
            max_tenant_concurrency: 1,
            wait_timeout: Duration::from_millis(50),
        },
    );
    let app = build_router(state.clone());
    assert_eq!(state.tenants.config().max_tenant_concurrency, 1);

    // Occupy the heavy tenant's only slot from outside the router, so the
    // request below deterministically finds the tenant at its cap.
    let held = {
        let tenants = Arc::clone(&state.tenants);
        tokio::task::spawn_blocking(move || tenants.acquire("heavy").unwrap())
            .await
            .unwrap()
    };

    let (status, body) = send(
        &app,
        Request::builder()
            .uri("/v1/events?account_id=heavy&after=0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "an over-cap tenant must be shed, not queued indefinitely"
    );
    let err: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err["code"], "UNAVAILABLE");

    // A different tenant is completely unaffected — the whole point.
    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/v1/events?account_id=quiet&after=0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Once the heavy tenant frees its slot it is served again.
    drop(held);
    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/v1/events?account_id=heavy&after=0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// The default policy leaves headroom for other tenants without penalising a
/// single-tenant self-host.
#[test]
fn the_default_tenant_cap_is_derived_from_the_pool() {
    let tmp = TempDb::new();
    let state = AppState::new(file_store(&tmp, 8));
    assert_eq!(state.tenants.config().max_tenant_concurrency, 7);
    assert_eq!(state.store.read_pool_size(), 8);

    // An in-memory store degenerates to one connection and one slot.
    let state = AppState::new(SyncStore::open_in_memory().unwrap());
    assert_eq!(state.tenants.config().max_tenant_concurrency, 1);
}

// ---------------------------------------------------------------------------
// No behavioral regression
// ---------------------------------------------------------------------------

/// A pooled, WAL-backed store must round-trip upload → push → pull exactly as
/// the single mutexed connection did, including dedup on `change_id`.
#[tokio::test]
async fn pooled_store_preserves_the_upload_push_pull_round_trip() {
    let tmp = TempDb::new();
    let app = build_router(AppState::new(file_store(&tmp, 4)));
    let account = "acct-roundtrip";

    let blob = b"opaque-ciphertext-bytes".to_vec();
    let hash = ct_hash(&blob);
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/{account}/{hash}"))
            .body(Body::from(blob))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let body = json!({
        "account_id": account,
        "events": [{
            "protocol_version": 1,
            "account_id": account,
            "device_id": "device-A",
            "change_id": "change-1",
            "key_epoch": 1,
            "blob_refs": [hash],
            "ciphertext_b64": STANDARD.encode(b"event-ciphertext"),
        }]
    });
    let (status, resp) = send(&app, json_req("POST", "/v1/events", &body)).await;
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(resp["high_water_seq"], 1);
    assert_eq!(resp["results"][0]["deduplicated"], false);

    // Re-pushing the same batch dedupes rather than duplicating.
    let (status, resp) = send(&app, json_req("POST", "/v1/events", &body)).await;
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(resp["results"][0]["deduplicated"], true);

    let (status, resp) = send(
        &app,
        Request::builder()
            .uri(format!("/v1/events?account_id={account}&after=0"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(resp["events"].as_array().unwrap().len(), 1);
    assert_eq!(resp["high_water_seq"], 1);
    assert_eq!(resp["next_cursor"], 1);
}

/// `/health` must report OK under load: a busy pool is transient capacity, not a
/// fault, and failing the container health check under load would make an
/// orchestrator restart a working server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_stays_ok_while_the_pool_is_busy() {
    let tmp = TempDb::new();
    let state = AppState::new(file_store(&tmp, 2));
    let app = build_router(state.clone());

    // Hold every pooled connection, proving the health check does not depend on
    // one being free.
    let acquired = Arc::new(Rendezvous::new(3));
    let release = Arc::new(Rendezvous::new(3));
    let holders: Vec<_> = (0..2)
        .map(|_| {
            let store = Arc::clone(&state.store);
            let acquired = Arc::clone(&acquired);
            let release = Arc::clone(&release);
            tokio::task::spawn_blocking(move || {
                store
                    .with_read_connection(|| {
                        acquired.wait(RENDEZVOUS_TIMEOUT) && release.wait(RENDEZVOUS_TIMEOUT)
                    })
                    .unwrap()
            })
        })
        .collect();

    let both_held = acquired.wait(RENDEZVOUS_TIMEOUT);
    let (status, body) = send(
        &app,
        Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // Always release before asserting, so a failure is a failure and not a hang.
    release.wait(RENDEZVOUS_TIMEOUT);
    let held_results = join_all(holders).await;

    assert!(both_held, "both pooled connections should have been held");
    assert!(held_results.into_iter().all(|ok| ok));
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp["status"], "ok");
}
