// SPDX-License-Identifier: Apache-2.0

//! Two-database convergence tests for the client sync engine, driven over the
//! in-memory [`MemoryTransport`] double (issue #126).
//!
//! These prove the acceptance criterion at the engine level: two independent
//! local databases, after syncing against a shared "server", converge to
//! byte-identical state — including per-field LWW, conflict-copy for concurrent
//! prose, observed-remove membership edges, and deletes.
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use pergamon_core::sync::event::{EntityType, Op};
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use pergamon_sync::{CryptoContext, MemoryBlobStore, MemoryTransport, SyncEngine};
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

fn read(db: &Database, et: EntityType, id: &str) -> Option<FieldMap> {
    db.read_entity_fields(et, id).unwrap()
}

#[test]
fn document_create_replicates_a_to_b() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields(&[
            ("title", json!("Hello")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
            ("content_text", json!("body v1")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();

    let stats = eng_a.sync(&db_a, &blob_a).unwrap();
    assert_eq!(stats.pushed, 1);

    let stats = eng_b.sync(&db_b, &blob_b).unwrap();
    assert_eq!(stats.applied, 1);

    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b);
    assert_eq!(b.get("title").unwrap(), &json!("Hello"));
    assert_eq!(b.get("content_text").unwrap(), &json!("body v1"));
}

#[test]
fn per_field_edits_on_both_devices_converge() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    // A creates and both sync so B has the doc.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields(&[
            ("title", json!("T")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    // A edits status (LWW), B edits title (LWW) — different fields, no conflict.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("status", json!("archived"))]),
        vec![],
        2_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("title", json!("Renamed"))]),
        vec![],
        2_050,
    )
    .unwrap();

    // Exchange both ways until stable.
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();

    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b, "devices must converge");
    assert_eq!(b.get("status").unwrap(), &json!("archived"));
    assert_eq!(b.get("title").unwrap(), &json!("Renamed"));
}

#[test]
fn concurrent_body_edits_conflict_copy_and_converge() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields(&[
            ("title", json!("T")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
            ("content_text", json!("base")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    // Concurrent edits to the same prose field, neither synced yet.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("content_text", json!("A wrote this"))]),
        vec![],
        2_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("content_text", json!("B wrote this"))]),
        vec![],
        2_000,
    )
    .unwrap();

    // Cross-sync until convergent.
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b, "concurrent prose edits must converge to one winner");

    // Each replica independently preserves the losing edit in its conflict
    // inbox, so the loser survives on both sides.
    let conflicts_a = db_a.list_conflicts(false).unwrap();
    let conflicts_b = db_b.list_conflicts(false).unwrap();
    assert_eq!(conflicts_a.len(), 1, "device A must preserve the loser");
    assert_eq!(conflicts_b.len(), 1, "device B must preserve the loser");
    // Both inboxes preserve the same losing text, and both live on the same winner.
    assert_eq!(conflicts_a[0].loser_value, conflicts_b[0].loser_value);
    let winner = a.get("content_text").unwrap();
    assert_ne!(winner.to_string(), conflicts_a[0].loser_value);
}

#[test]
fn tag_membership_edge_replicates() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    db_a.emit_change(
        EntityType::Document,
        "d1",
        Op::Upsert,
        fields(&[
            ("title", json!("Doc")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();
    db_a.emit_change(
        EntityType::Tag,
        "t1",
        Op::Upsert,
        fields(&[("name", json!("fav"))]),
        vec![],
        1_010,
    )
    .unwrap();
    db_a.emit_change(
        EntityType::TagEdge,
        "d1:t1",
        Op::Upsert,
        fields(&[("present", json!(true))]),
        vec![],
        1_020,
    )
    .unwrap();

    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    assert!(
        db_b.set_edge(EntityType::TagEdge, "d1:t1")
            .unwrap()
            .is_present()
    );
    assert!(read(&db_b, EntityType::Tag, "t1").is_some());

    // B removes membership; A converges to absent.
    db_b.emit_change(
        EntityType::TagEdge,
        "d1:t1",
        Op::Upsert,
        fields(&[("present", json!(false))]),
        vec![],
        2_000,
    )
    .unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();

    assert!(
        !db_a
            .set_edge(EntityType::TagEdge, "d1:t1")
            .unwrap()
            .is_present()
    );
    assert!(
        !db_b
            .set_edge(EntityType::TagEdge, "d1:t1")
            .unwrap()
            .is_present()
    );
}

#[test]
fn delete_replicates_and_removes_entity() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    db_a.emit_change(
        EntityType::Tag,
        "t9",
        Op::Upsert,
        fields(&[("name", json!("temp"))]),
        vec![],
        1_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    assert!(read(&db_b, EntityType::Tag, "t9").is_some());

    db_a.emit_change(
        EntityType::Tag,
        "t9",
        Op::Delete,
        FieldMap::new(),
        vec![],
        2_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    assert!(read(&db_b, EntityType::Tag, "t9").is_none());
}

#[test]
fn re_pull_is_idempotent() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    db_a.emit_change(
        EntityType::Tag,
        "t1",
        Op::Upsert,
        fields(&[("name", json!("x"))]),
        vec![],
        1_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();

    let first = eng_b.sync(&db_b, &blob_b).unwrap();
    assert_eq!(first.applied, 1);
    // A second pull with nothing new applies nothing.
    let second = eng_b.sync(&db_b, &blob_b).unwrap();
    assert_eq!(second.applied, 0);
}
