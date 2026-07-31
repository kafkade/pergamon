// SPDX-License-Identifier: AGPL-3.0-only

//! Opt-in load benchmark for the sync server's concurrency work (WP-3e, [#201]).
//!
//! This is the "load test demonstrating the improvement" the issue asks for. It
//! is `#[ignore]`d so a normal `cargo test` run never pays for it and CI can
//! never flake on it — it asserts only that the workload completes correctly and
//! **prints** the timings for a human to compare.
//!
//! Run it with:
//!
//! ```sh
//! cargo test -p pergamon-sync-server --release --test load_concurrency \
//!     -- --ignored --nocapture
//! ```
//!
//! `--release` matters: a debug build inflates per-request CPU everywhere and
//! blurs the comparison.
//!
//! ## How the before/after comparison works
//! Both halves run in the same process against the same code. The "before"
//! configuration is a store with `read_pool_size = 1`, which reproduces exactly
//! the pre-WP-3e topology: every read serialized behind a single connection. The
//! "after" configuration is the default pool. No separate baseline build or
//! `git stash` is needed, and both numbers come from the same machine under the
//! same conditions.
//!
//! Two levels are reported:
//!
//! - **store level** — concurrent readers calling the store directly. This
//!   isolates the component that used to serialize and is the number that
//!   answers the acceptance criterion.
//! - **HTTP level** — the same workload through the real router. Necessarily a
//!   smaller ratio, because per-request HTTP/JSON overhead is untouched by this
//!   work and dilutes the store's share, but it is what an operator experiences.
//!
//! ## Choice of workload, and why it is not blob downloads
//! The benchmark pulls **event pages** (`GET /v1/events`), the hot read path of
//! the ADR-022 sync protocol: a client pulls pages of envelopes on every sync.
//! Each pull decodes hundreds of rows inside the connection, so the work is CPU
//! bound *while the connection is held* — exactly the thing a single connection
//! serialized.
//!
//! Large blob downloads were tried first and rejected as a misleading workload:
//! a multi-megabyte `blob_get` is dominated by one big memory copy, so it
//! saturates memory bandwidth rather than the connection, and it under-reports
//! the pool's effect for reasons that have nothing to do with locking.
//!
//! ## What it does *not* claim
//! `SQLite` allows exactly one writer at a time even in WAL mode, so the write
//! benchmark is reported for honesty, not as a speed-up: writes serialize on the
//! store's single writer connection by design. See ADR-031.
//!
//! [#201]: https://github.com/kafkade/pergamon/issues/201

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pergamon_sync_server::store::EventRecord;
use pergamon_sync_server::{AppState, FairnessConfig, PoolConfig, SyncStore, build_router};
use serde_json::json;
use tower::ServiceExt;

/// Tenants driven concurrently.
const TENANTS: usize = 8;
/// Event pulls each tenant issues.
const PULLS_PER_TENANT: usize = 40;
/// Events returned per pull (the protocol's default page size).
const PAGE_SIZE: u32 = 500;
/// Events seeded per tenant.
const EVENTS_PER_TENANT: usize = 2_000;
/// Ciphertext size per seeded event — small, so the benchmark measures row
/// decoding rather than memory bandwidth.
const CIPHERTEXT_BYTES: usize = 256;
/// Reader-pool size for the "after" configuration.
const POOL_SIZE: usize = 8;

/// A unique temp path for a benchmark database, cleaned up on drop.
struct TempDb {
    path: std::path::PathBuf,
}

impl TempDb {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pergamon-sync-load-{tag}-{}.db",
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

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

/// Open a store with the given reader-pool size.
fn open_store(tmp: &TempDb, pool_size: usize) -> SyncStore {
    SyncStore::open_with_pool(
        &tmp.path,
        PoolConfig {
            size: pool_size,
            checkout_timeout: Duration::from_secs(30),
        },
    )
    .unwrap()
}

/// Seed every tenant's event log.
fn seed(store: &SyncStore) {
    for tenant in 0..TENANTS {
        let account = format!("tenant-{tenant}");
        let batch: Vec<EventRecord> = (0..EVENTS_PER_TENANT)
            .map(|i| EventRecord {
                protocol_version: 1,
                account_id: account.clone(),
                device_id: "device-A".to_owned(),
                change_id: format!("change-{i}"),
                entity_ref: Some(format!("entity-{}", i % 64)),
                key_epoch: 1,
                blob_refs: Vec::new(),
                ciphertext: vec![u8::try_from(i % 251).unwrap(); CIPHERTEXT_BYTES],
                signature: vec![9_u8; 64],
            })
            .collect();
        store.push_events(&account, &batch).unwrap();
    }
}

/// Wrap a store in the real router.
///
/// No per-tenant cap: the benchmark measures the pool, and a cap would conflate
/// two effects. Fairness has its own test in `e2e_concurrency.rs`.
fn router_for(store: SyncStore) -> Router {
    build_router(AppState::with_fairness(store, FairnessConfig::disabled()))
}

/// The cursor for pull `i`, walking the log so pages differ between calls.
fn cursor_for(i: usize) -> u64 {
    ((i * usize::try_from(PAGE_SIZE).unwrap()) % (EVENTS_PER_TENANT - PAGE_SIZE as usize)) as u64
}

/// `TENANTS` concurrent readers pulling event pages **straight from the store**.
async fn store_pull_load(store: &Arc<SyncStore>) -> Duration {
    let started = Instant::now();
    let tasks: Vec<_> = (0..TENANTS)
        .map(|tenant| {
            let store = Arc::clone(store);
            tokio::task::spawn_blocking(move || {
                let account = format!("tenant-{tenant}");
                for i in 0..PULLS_PER_TENANT {
                    let (events, _high_water) =
                        store.pull_page(&account, cursor_for(i), PAGE_SIZE).unwrap();
                    assert_eq!(events.len(), PAGE_SIZE as usize);
                }
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    started.elapsed()
}

/// The same workload through `GET /v1/events`.
async fn http_pull_load(app: &Router) -> Duration {
    let started = Instant::now();
    let tasks: Vec<_> = (0..TENANTS)
        .map(|tenant| {
            let app = app.clone();
            tokio::spawn(async move {
                let account = format!("tenant-{tenant}");
                for i in 0..PULLS_PER_TENANT {
                    let cursor = cursor_for(i);
                    let (status, _) = send(
                        &app,
                        Request::builder()
                            .uri(format!(
                                "/v1/events?account_id={account}&after={cursor}&limit={PAGE_SIZE}"
                            ))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await;
                    assert_eq!(status, StatusCode::OK);
                }
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    started.elapsed()
}

/// `TENANTS` concurrent writers pushing events through the router.
async fn http_push_load(app: &Router) -> Duration {
    let started = Instant::now();
    let tasks: Vec<_> = (0..TENANTS)
        .map(|tenant| {
            let app = app.clone();
            tokio::spawn(async move {
                let account = format!("tenant-{tenant}");
                for i in 0..PULLS_PER_TENANT {
                    let body = json!({
                        "account_id": account,
                        "events": [{
                            "protocol_version": 1,
                            "account_id": account,
                            "device_id": "device-A",
                            "change_id": format!("push-{i}"),
                            "key_epoch": 1,
                            "blob_refs": [],
                            "ciphertext_b64": STANDARD.encode(vec![7_u8; CIPHERTEXT_BYTES]),
                        }]
                    });
                    let (status, _) = send(
                        &app,
                        Request::builder()
                            .method("POST")
                            .uri("/v1/events")
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(serde_json::to_vec(&body).unwrap()))
                            .unwrap(),
                    )
                    .await;
                    assert_eq!(status, StatusCode::OK);
                }
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    started.elapsed()
}

/// Operations per second for a run.
///
/// Benchmark counters are in the hundreds, far below `f64`'s exact-integer
/// range, so the cast is lossless here.
#[allow(clippy::cast_precision_loss)]
fn ops(total: usize, elapsed: Duration) -> f64 {
    total as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
}

/// `before / after` as a speed-up factor.
fn speedup(before: Duration, after: Duration) -> f64 {
    before.as_secs_f64() / after.as_secs_f64().max(f64::EPSILON)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "load benchmark; run with --release ... -- --ignored --nocapture"]
async fn load_concurrent_tenants_before_and_after() {
    let total = TENANTS * PULLS_PER_TENANT;
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZeroUsize::get);

    // --- Store level ------------------------------------------------------
    // "Before": one connection — exactly the pre-WP-3e topology.
    let serial_db = TempDb::new("store-serial");
    let serial_store = open_store(&serial_db, 1);
    seed(&serial_store);
    let serial_store = Arc::new(serial_store);
    store_pull_load(&serial_store).await; // warm-up
    let store_before = store_pull_load(&serial_store).await;

    // "After": the default reader pool.
    let pooled_db = TempDb::new("store-pooled");
    let pooled_store = open_store(&pooled_db, POOL_SIZE);
    seed(&pooled_store);
    let pooled_store = Arc::new(pooled_store);
    store_pull_load(&pooled_store).await; // warm-up
    let store_after = store_pull_load(&pooled_store).await;

    // --- HTTP level -------------------------------------------------------
    let http_serial_db = TempDb::new("http-serial");
    let seed_store = open_store(&http_serial_db, 1);
    seed(&seed_store);
    drop(seed_store);
    let serial_app = router_for(open_store(&http_serial_db, 1));
    http_pull_load(&serial_app).await; // warm-up
    let http_before = http_pull_load(&serial_app).await;

    let http_pooled_db = TempDb::new("http-pooled");
    let seed_store = open_store(&http_pooled_db, 1);
    seed(&seed_store);
    drop(seed_store);
    let pooled_app = router_for(open_store(&http_pooled_db, POOL_SIZE));
    http_pull_load(&pooled_app).await; // warm-up
    let http_after = http_pull_load(&pooled_app).await;

    // --- Write path -------------------------------------------------------
    let write_serial_db = TempDb::new("write-serial");
    let write_serial = router_for(open_store(&write_serial_db, 1));
    let writes_before = http_push_load(&write_serial).await;

    let write_pooled_db = TempDb::new("write-pooled");
    let write_pooled = router_for(open_store(&write_pooled_db, POOL_SIZE));
    let writes_after = http_push_load(&write_pooled).await;

    println!();
    println!("=== WP-3e (#201) sync-server concurrency load test ===");
    println!("host:     {cores} logical cores");
    println!(
        "workload: {TENANTS} concurrent tenants x {PULLS_PER_TENANT} event pulls = {total} pulls, \
         {PAGE_SIZE} events per page"
    );
    println!("          (each tenant's log holds {EVENTS_PER_TENANT} events)");
    println!();
    println!("READ PATH, STORE LEVEL — the component that used to serialize");
    println!(
        "  before  1 connection (pre-WP-3e):     {store_before:>10.3?}   {:>8.0} pulls/s",
        ops(total, store_before)
    );
    println!(
        "  after   WAL + {POOL_SIZE}-connection pool:    {store_after:>10.3?}   {:>8.0} pulls/s",
        ops(total, store_after)
    );
    println!("  speed-up: {:.2}x", speedup(store_before, store_after));
    println!();
    println!("READ PATH, HTTP LEVEL — end to end through the real router");
    println!(
        "  before  1 connection (pre-WP-3e):     {http_before:>10.3?}   {:>8.0} req/s",
        ops(total, http_before)
    );
    println!(
        "  after   WAL + {POOL_SIZE}-connection pool:    {http_after:>10.3?}   {:>8.0} req/s",
        ops(total, http_after)
    );
    println!("  speed-up: {:.2}x", speedup(http_before, http_after));
    println!("  (lower than the store-level ratio by construction: per-request HTTP,");
    println!("   JSON and base64 work is untouched here and dilutes the store's share.)");
    println!();
    println!("WRITE PATH — reported for honesty, NOT as a win");
    println!(
        "  before  1 connection (pre-WP-3e):     {writes_before:>10.3?}   {:>8.0} req/s",
        ops(total, writes_before)
    );
    println!(
        "  after   WAL + {POOL_SIZE}-connection pool:    {writes_after:>10.3?}   {:>8.0} req/s",
        ops(total, writes_after)
    );
    println!("  ratio:    {:.2}x", speedup(writes_before, writes_after));
    println!("  SQLite allows exactly ONE writer at a time even in WAL mode, so writes");
    println!("  serialize on the store's single writer connection by design. This number");
    println!("  is expected to be roughly flat; pooling is a read-path win. See ADR-031.");
    println!();

    // The only assertions are correctness ones: no wall-clock threshold, so this
    // can never flake on a loaded machine.
    for elapsed in [
        store_before,
        store_after,
        http_before,
        http_after,
        writes_before,
        writes_after,
    ] {
        assert!(elapsed > Duration::ZERO);
    }
}
