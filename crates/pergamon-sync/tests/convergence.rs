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

// ======================================================================
// ADR-023 typed conflict policies (issue #127)
// ======================================================================

/// Emit a highlight + its review card so a review log has somewhere to land.
/// (`review_logs.card_id` -> `review_cards.id` -> `highlight_meta` -> `content_items`.)
fn seed_review_card(db: &Database, highlight_id: &str, card_id: &str, now: u64) {
    db.emit_change(
        EntityType::Highlight,
        highlight_id,
        Op::Upsert,
        fields(&[("quote_text", json!("a quote")), ("note", json!(""))]),
        vec![],
        now,
    )
    .unwrap();
    db.emit_change(
        EntityType::ReviewCard,
        card_id,
        Op::Upsert,
        fields(&[("content_item_id", json!(highlight_id))]),
        vec![],
        now + 1,
    )
    .unwrap();
}

/// A review-log field map with every NOT NULL column filled. Only `rating` and
/// `reviewed_at` drive the derived merge; the rest are recomputed by FSRS.
fn review_log(card_id: &str, rating: i64, reviewed_at: &str) -> FieldMap {
    fields(&[
        ("card_id", json!(card_id)),
        ("rating", json!(rating)),
        ("state_before", json!("new")),
        ("stability_before", Value::Null),
        ("difficulty_before", Value::Null),
        ("state_after", json!("review")),
        ("stability_after", json!(1.0)),
        ("difficulty_after", json!(5.0)),
        ("elapsed_days", json!(0.0)),
        ("scheduled_days", json!(1.0)),
        ("reviewed_at", json!(reviewed_at)),
    ])
}

fn card_int(db: &Database, card_id: &str, field: &str) -> i64 {
    read(db, EntityType::ReviewCard, card_id)
        .unwrap()
        .get(field)
        .unwrap()
        .as_i64()
        .unwrap()
}

/// Two devices review the same card concurrently. The card's schedule is
/// *derived* from the review-log union, so both converge and the due counts
/// reflect every review — never doubled, never dropped (issue #127 acceptance).
#[test]
fn review_concurrent_reviews_merge_due_counts() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    // A seeds a highlight, its card, and one "Good" review, then both sync.
    seed_review_card(&db_a, "h1", "c1", 1_000);
    db_a.emit_change(
        EntityType::ReviewLog,
        "log-a1",
        Op::Upsert,
        review_log("c1", 3, "2024-01-01T00:00:00Z"),
        vec![],
        1_010,
    )
    .unwrap();
    db_a.recompute_review_card("c1").unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    assert_eq!(card_int(&db_a, "c1", "review_count"), 1);
    assert_eq!(card_int(&db_b, "c1", "review_count"), 1);

    // Concurrent reviews: A rates Good again, B rates Again (a lapse). Neither
    // has seen the other's log yet.
    db_a.emit_change(
        EntityType::ReviewLog,
        "log-a2",
        Op::Upsert,
        review_log("c1", 3, "2024-01-03T00:00:00Z"),
        vec![],
        2_000,
    )
    .unwrap();
    db_a.recompute_review_card("c1").unwrap();
    db_b.emit_change(
        EntityType::ReviewLog,
        "log-b1",
        Op::Upsert,
        review_log("c1", 1, "2024-01-02T00:00:00Z"),
        vec![],
        2_010,
    )
    .unwrap();
    db_b.recompute_review_card("c1").unwrap();

    // Cross-sync until stable.
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    let a = read(&db_a, EntityType::ReviewCard, "c1").unwrap();
    let b = read(&db_b, EntityType::ReviewCard, "c1").unwrap();
    assert_eq!(a, b, "derived review state must converge");
    // Union of all three reviews: 3 total, 1 lapse (the Again).
    assert_eq!(card_int(&db_a, "c1", "review_count"), 3);
    assert_eq!(card_int(&db_a, "c1", "lapse_count"), 1);
    // No conflict inbox entry: review state auto-merges by derivation.
    assert_eq!(db_a.list_conflicts(false).unwrap().len(), 0);
    assert_eq!(db_b.list_conflicts(false).unwrap().len(), 0);
}

/// Concurrent edits to a note's authored body converge to one winner and both
/// replicas preserve the loser in their conflict inbox (conflict-copy).
#[test]
fn note_concurrent_body_edits_conflict_and_converge() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    // A parent document is required for the note's foreign key.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields(&[("title", json!("Doc")), ("content_type", json!("article"))]),
        vec![],
        1_000,
    )
    .unwrap();
    db_a.emit_change(
        EntityType::Note,
        "n1",
        Op::Upsert,
        fields(&[("content_item_id", json!("doc-1")), ("body", json!("base"))]),
        vec![],
        1_010,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    assert!(read(&db_b, EntityType::Note, "n1").is_some());

    // Concurrent body edits, same clock, neither synced.
    db_a.emit_change(
        EntityType::Note,
        "n1",
        Op::FieldPatch,
        fields(&[("body", json!("A body"))]),
        vec![],
        2_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Note,
        "n1",
        Op::FieldPatch,
        fields(&[("body", json!("B body"))]),
        vec![],
        2_000,
    )
    .unwrap();

    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    let a = read(&db_a, EntityType::Note, "n1").unwrap();
    let b = read(&db_b, EntityType::Note, "n1").unwrap();
    assert_eq!(a, b, "concurrent note bodies must converge to one winner");
    let conflicts_a = db_a.list_conflicts(false).unwrap();
    let conflicts_b = db_b.list_conflicts(false).unwrap();
    assert_eq!(conflicts_a.len(), 1, "device A preserves the losing body");
    assert_eq!(conflicts_b.len(), 1, "device B preserves the losing body");
    assert_eq!(conflicts_a[0].loser_value, conflicts_b[0].loser_value);
}

/// A delete concurrent with an edit the deleter never observed must not silently
/// erase the authored prose: the edit is preserved in the conflict inbox on both
/// devices while the entity stays deleted (ADR-023 resurrect-as-conflict).
#[test]
fn note_delete_vs_unseen_edit_preserves_prose() {
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
        fields(&[("title", json!("Doc")), ("content_type", json!("article"))]),
        vec![],
        1_000,
    )
    .unwrap();
    db_a.emit_change(
        EntityType::Note,
        "n1",
        Op::Upsert,
        fields(&[("content_item_id", json!("doc-1")), ("body", json!("base"))]),
        vec![],
        1_010,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    // A deletes the note; B concurrently edits its body (A never sees the edit).
    db_a.emit_change(
        EntityType::Note,
        "n1",
        Op::Delete,
        FieldMap::new(),
        vec![],
        2_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Note,
        "n1",
        Op::FieldPatch,
        fields(&[("body", json!("B kept editing"))]),
        vec![],
        2_050,
    )
    .unwrap();

    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    // Delete wins live on both sides — the note is gone.
    assert!(read(&db_a, EntityType::Note, "n1").is_none());
    assert!(read(&db_b, EntityType::Note, "n1").is_none());
    // But the unseen prose edit survives in the conflict inbox on both devices.
    let conflicts_a = db_a.list_conflicts(false).unwrap();
    let conflicts_b = db_b.list_conflicts(false).unwrap();
    assert_eq!(conflicts_a.len(), 1, "A preserves the unseen edit");
    assert_eq!(conflicts_b.len(), 1, "B preserves the unseen edit");
    assert_eq!(conflicts_a[0].loser_value, conflicts_b[0].loser_value);
    assert!(conflicts_a[0].loser_value.contains("B kept editing"));
}

/// A normal delete the deleter fully observed raises no false conflict.
#[test]
fn note_observed_delete_raises_no_conflict() {
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
        fields(&[("title", json!("Doc")), ("content_type", json!("article"))]),
        vec![],
        1_000,
    )
    .unwrap();
    db_a.emit_change(
        EntityType::Note,
        "n1",
        Op::Upsert,
        fields(&[("content_item_id", json!("doc-1")), ("body", json!("base"))]),
        vec![],
        1_010,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    // A deletes after observing the current body; no concurrent edit anywhere.
    db_a.emit_change(
        EntityType::Note,
        "n1",
        Op::Delete,
        FieldMap::new(),
        vec![],
        2_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    assert!(read(&db_b, EntityType::Note, "n1").is_none());
    assert_eq!(db_a.list_conflicts(false).unwrap().len(), 0);
    assert_eq!(db_b.list_conflicts(false).unwrap().len(), 0);
}

/// Distinct new annotations authored independently on two devices both survive
/// the merge (append semantics — no accidental overwrite).
#[test]
fn distinct_new_notes_from_both_devices_survive() {
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
        fields(&[("title", json!("Doc")), ("content_type", json!("article"))]),
        vec![],
        1_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    db_a.emit_change(
        EntityType::Note,
        "note-a",
        Op::Upsert,
        fields(&[
            ("content_item_id", json!("doc-1")),
            ("body", json!("from A")),
        ]),
        vec![],
        2_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Note,
        "note-b",
        Op::Upsert,
        fields(&[
            ("content_item_id", json!("doc-1")),
            ("body", json!("from B")),
        ]),
        vec![],
        2_010,
    )
    .unwrap();

    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();

    for db in [&db_a, &db_b] {
        assert!(read(db, EntityType::Note, "note-a").is_some());
        assert!(read(db, EntityType::Note, "note-b").is_some());
    }
    assert_eq!(db_a.list_conflicts(false).unwrap().len(), 0);
}

/// Collections: a concurrent rename is per-field LWW (no conflict inbox entry),
/// and concurrent membership adds set-union so both survive.
#[test]
fn collection_rename_lww_and_membership_union() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let eng_a = engine(&transport, "device-a");
    let eng_b = engine(&transport, "device-b");

    db_a.emit_change(
        EntityType::Collection,
        "col-1",
        Op::Upsert,
        fields(&[("name", json!("Reading"))]),
        vec![],
        1_000,
    )
    .unwrap();
    for (id, ts) in [("d1", 1_010), ("d2", 1_020)] {
        db_a.emit_change(
            EntityType::Document,
            id,
            Op::Upsert,
            fields(&[("title", json!(id)), ("content_type", json!("article"))]),
            vec![],
            ts,
        )
        .unwrap();
    }
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    // Concurrent rename (LWW) — B's later clock wins, no conflict surfaced.
    db_a.emit_change(
        EntityType::Collection,
        "col-1",
        Op::FieldPatch,
        fields(&[("name", json!("A name"))]),
        vec![],
        2_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Collection,
        "col-1",
        Op::FieldPatch,
        fields(&[("name", json!("B name"))]),
        vec![],
        2_100,
    )
    .unwrap();
    // Concurrent membership adds of different docs — both must survive.
    db_a.emit_change(
        EntityType::CollectionEdge,
        "d1:col-1",
        Op::Upsert,
        fields(&[("present", json!(true))]),
        vec![],
        2_010,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::CollectionEdge,
        "d2:col-1",
        Op::Upsert,
        fields(&[("present", json!(true))]),
        vec![],
        2_020,
    )
    .unwrap();

    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    let a = read(&db_a, EntityType::Collection, "col-1").unwrap();
    let b = read(&db_b, EntityType::Collection, "col-1").unwrap();
    assert_eq!(a, b, "collection rows must converge");
    assert_eq!(
        a.get("name").unwrap(),
        &json!("B name"),
        "later rename wins (LWW)"
    );
    for db in [&db_a, &db_b] {
        assert!(
            db.set_edge(EntityType::CollectionEdge, "d1:col-1")
                .unwrap()
                .is_present()
        );
        assert!(
            db.set_edge(EntityType::CollectionEdge, "d2:col-1")
                .unwrap()
                .is_present()
        );
    }
    // A rename is low-stakes LWW, not authored prose: never a conflict entry.
    assert_eq!(db_a.list_conflicts(false).unwrap().len(), 0);
    assert_eq!(db_b.list_conflicts(false).unwrap().len(), 0);
}

/// Read/triage state is the highest-churn, lowest-stakes field: concurrent
/// status flips resolve by LWW with no conflict-inbox entry.
#[test]
fn read_state_flips_lww_no_conflict() {
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
            ("title", json!("Doc")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    // Concurrent triage flips of the same field.
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
        fields(&[("status", json!("reading"))]),
        vec![],
        2_100,
    )
    .unwrap();

    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();
    eng_a.sync(&db_a, &blob_a).unwrap();
    eng_b.sync(&db_b, &blob_b).unwrap();

    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b, "status must converge");
    assert_eq!(
        a.get("status").unwrap(),
        &json!("reading"),
        "later flip wins (LWW)"
    );
    assert_eq!(db_a.list_conflicts(false).unwrap().len(), 0);
    assert_eq!(db_b.list_conflicts(false).unwrap().len(), 0);
}

/// A remote sync peer's change body carries a free-form field map whose keys are
/// interpolated as SQL column identifiers by the storage upsert path. A crafted
/// key that is not a real column for the entity must be rejected before any SQL
/// runs, closing the identifier-injection vector (ADR-023 hardening).
#[test]
fn malicious_sync_column_name_is_rejected() {
    let db = synced_db("device-a");

    // Seed a benign secret in another table to prove it cannot be exfiltrated
    // or clobbered via an injected sub-assignment.
    db.emit_change(
        EntityType::Settings,
        "secret",
        Op::Upsert,
        fields(&[("value", json!("s3cr3t"))]),
        vec![],
        500,
    )
    .unwrap();

    // If this key were interpolated raw, the statement would become
    // `UPDATE content_items SET content_text = (SELECT value FROM settings ...),
    //  title = ?2 ...` — a cross-table read plus an unauthorized column write.
    let malicious = fields(&[(
        "content_text = (SELECT value FROM settings LIMIT 1), title",
        json!("x"),
    )]);
    let result = db.write_entity_fields(EntityType::Document, "doc-x", &malicious, Op::Upsert);
    assert!(
        result.is_err(),
        "unknown/injected sync column name must be rejected"
    );

    // The document was never created, and the secret is untouched.
    assert!(read(&db, EntityType::Document, "doc-x").is_none());
    assert_eq!(
        read(&db, EntityType::Settings, "secret")
            .unwrap()
            .get("value")
            .unwrap(),
        &json!("s3cr3t"),
    );
}

/// A review log with an extreme `reviewed_at_ms` (which a malicious peer can set
/// via the `reviewed_at` field) must not panic the derived-merge fold through
/// `i64` overflow — the schedule arithmetic saturates instead.
#[test]
fn extreme_review_timestamp_does_not_panic_fold() {
    use pergamon_core::fsrs::{Rating, Scheduler};
    use pergamon_core::sync::review::{ReviewLogEntry, derive_card_state};

    let logs = vec![
        ReviewLogEntry {
            id: "l1".to_owned(),
            reviewed_at_ms: i64::MIN,
            rating: Rating::Good,
        },
        ReviewLogEntry {
            id: "l2".to_owned(),
            reviewed_at_ms: i64::MAX,
            rating: Rating::Again,
        },
    ];
    // Must fold without panicking (saturating arithmetic).
    let derived = derive_card_state(&Scheduler::default_v5(), &logs);
    assert_eq!(derived.review_count, 2);
}
