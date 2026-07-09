// SPDX-License-Identifier: Apache-2.0

//! Acceptance test for background sync (issue #129).
//!
//! Proves the issue's acceptance criterion — "an item updated on one device
//! appears on another after sync without manual steps" — at the driver level:
//! two independent databases are advanced **only** by the background
//! [`run_forever`] loop (with its [`SyncScheduler`]/[`BackoffPolicy`]), never by
//! a direct `push`/`pull`/`sync` call, and still converge.
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::duration_suboptimal_units)]

use std::cell::RefCell;
use std::time::Duration;

use pergamon_core::sync::event::{EntityType, Op};
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use pergamon_sync::{
    BackoffPolicy, CryptoContext, Jitter, MemoryBlobStore, MemoryTransport, RoundOutcome,
    RoundReport, Sleeper, SyncEngine, SyncScheduler, Wake, run_forever,
};
use serde_json::{Value, json};

const ACCOUNT_HEX_SEED: [u8; 16] = [3u8; 16];
const ARK_SEED: [u8; 32] = [7u8; 32];

fn account_hex() -> String {
    AccountId::from_bytes(ACCOUNT_HEX_SEED).to_hex()
}

fn crypto(device: &str) -> CryptoContext {
    CryptoContext::new(
        AccountRootKey::from_bytes(ARK_SEED),
        account_hex(),
        device.to_owned(),
        0,
    )
    .unwrap()
}

fn engine(transport: &MemoryTransport, device: &str) -> SyncEngine<MemoryTransport> {
    SyncEngine::new(transport.clone(), crypto(device))
}

fn synced_db(device: &str) -> Database {
    let db = Database::open_in_memory().unwrap();
    db.set_sync_identity(&account_hex(), device, 0, Some("mem://test"))
        .unwrap();
    db
}

fn fields(pairs: &[(&str, Value)]) -> FieldMap {
    let mut m = FieldMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    m
}

/// A [`Sleeper`] that stops the loop as soon as it is asked to wait, so
/// [`run_forever`] runs **exactly one** round (a round always precedes the
/// first wait) and returns cleanly.
struct StopAfterRound;

impl Sleeper for StopAfterRound {
    fn wait(&self, _dur: Duration) -> Wake {
        Wake::Shutdown
    }
}

fn default_scheduler() -> SyncScheduler {
    let backoff = BackoffPolicy::new(Duration::from_secs(5), Duration::from_secs(300), 2.0);
    SyncScheduler::new(Duration::from_secs(300), backoff)
}

/// Run one background round via `run_forever`, returning the reports observed.
fn run_one_round(engine: &SyncEngine<MemoryTransport>, db: &Database) -> Vec<RoundReport> {
    let blobs = MemoryBlobStore::new();
    let reports = RefCell::new(Vec::new());
    run_forever(
        || engine.sync(db, &blobs),
        default_scheduler(),
        &StopAfterRound,
        Jitter::default(),
        |report: &RoundReport| reports.borrow_mut().push(report.clone()),
    )
    .unwrap();
    reports.into_inner()
}

fn read(db: &Database, et: EntityType, id: &str) -> Option<FieldMap> {
    db.read_entity_fields(et, id).unwrap()
}

#[test]
fn background_loop_propagates_edit_without_manual_sync() {
    let transport = MemoryTransport::new();
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    // Device A saves an item locally (as a triage/save mutation would).
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields(&[
            ("title", json!("Background hello")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
            ("content_text", json!("body v1")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();

    // The background loop on A pushes it — no manual push call.
    let a_reports = run_one_round(&eng_a, &db_a);
    assert_eq!(a_reports.len(), 1, "one round should run before shutdown");
    match &a_reports[0].outcome {
        RoundOutcome::Synced(stats) => assert_eq!(stats.pushed, 1),
        RoundOutcome::Offline(msg) => panic!("expected a synced round, got offline: {msg}"),
    }
    // A healthy round schedules the next wake at the configured interval.
    assert_eq!(a_reports[0].consecutive_failures, 0);
    assert_eq!(a_reports[0].next_delay, Duration::from_secs(300));

    // The background loop on B pulls and applies it — no manual pull call.
    let b_reports = run_one_round(&eng_b, &db_b);
    match &b_reports[0].outcome {
        RoundOutcome::Synced(stats) => assert_eq!(stats.applied, 1),
        RoundOutcome::Offline(msg) => panic!("expected a synced round, got offline: {msg}"),
    }

    // The item edited on A now appears on B, byte-identical.
    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b);
    assert_eq!(b.get("title").unwrap(), &json!("Background hello"));
    assert_eq!(b.get("content_text").unwrap(), &json!("body v1"));
}

#[test]
fn background_loop_converges_a_later_edit() {
    let transport = MemoryTransport::new();
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields(&[
            ("title", json!("v1")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();
    run_one_round(&eng_a, &db_a);
    run_one_round(&eng_b, &db_b);

    // A later status change on A propagates to B through the loop alone.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("status", json!("archived"))]),
        vec![],
        2_000,
    )
    .unwrap();
    run_one_round(&eng_a, &db_a);
    run_one_round(&eng_b, &db_b);

    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(b.get("status").unwrap(), &json!("archived"));
}
