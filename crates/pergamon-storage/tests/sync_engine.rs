//! Round-trip tests for the storage half of the sync engine (issue #126).
//!
//! These exercise the V13 tables and their APIs directly: the outbox, per-field
//! clocks, observed-remove set edges, tombstones, the applied guard, the
//! conflict inbox, and the `emit_change` tracked-write path.
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use pergamon_core::sync::event::{EntityType, Op};
use pergamon_core::sync::hlc::Hlc;
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use serde_json::json;

fn test_db() -> Database {
    Database::open_in_memory().unwrap_or_else(|e| unreachable!("open in-memory DB: {e}"))
}

/// A db with sync enabled (account/device identity set).
fn synced_db() -> Database {
    let db = test_db();
    db.set_sync_identity("acct-hex", "device-a", 0, Some("http://localhost"))
        .unwrap();
    db
}

fn fields(pairs: &[(&str, serde_json::Value)]) -> FieldMap {
    let mut m = FieldMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    m
}

#[test]
fn emit_without_sync_writes_canonical_only() {
    let db = test_db();
    let f = fields(&[
        ("title", json!("Hello")),
        ("content_type", json!("article")),
        ("status", json!("inbox")),
    ]);
    let change_id = db
        .emit_change(EntityType::Document, "doc-1", Op::Upsert, f, vec![], 1_000)
        .unwrap();
    assert!(change_id.is_none(), "no outbox when sync disabled");
    assert_eq!(db.pending_outbox_count().unwrap(), 0);

    let read = db
        .read_entity_fields(EntityType::Document, "doc-1")
        .unwrap()
        .unwrap();
    assert_eq!(read.get("title").unwrap(), &json!("Hello"));
}

#[test]
fn emit_with_sync_enqueues_outbox_and_stamps_clock() {
    let db = synced_db();
    let f = fields(&[
        ("title", json!("Tracked")),
        ("content_type", json!("article")),
        ("status", json!("inbox")),
    ]);
    let change_id = db
        .emit_change(EntityType::Document, "doc-2", Op::Upsert, f, vec![], 2_000)
        .unwrap();
    assert!(change_id.is_some());
    assert_eq!(db.pending_outbox_count().unwrap(), 1);

    let pending = db.pending_outbox(10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entity_id, "doc-2");
    assert_eq!(pending[0].entity_type, EntityType::Document);

    // Each field carries a clock.
    let clock = db
        .entity_clock(EntityType::Document, "doc-2", "title")
        .unwrap();
    assert!(clock.is_some());
    assert_eq!(clock.unwrap().device_id, "device-a");
}

#[test]
fn outbox_ack_clears_pending() {
    let db = synced_db();
    let f = fields(&[("name", json!("rust"))]);
    db.emit_change(EntityType::Tag, "tag-1", Op::Upsert, f, vec![], 3_000)
        .unwrap();
    let pending = db.pending_outbox(10).unwrap();
    assert_eq!(pending.len(), 1);
    db.mark_outbox_acked(&pending[0].change_id, 42).unwrap();
    assert_eq!(db.pending_outbox_count().unwrap(), 0);
}

#[test]
fn field_patch_updates_only_provided_columns() {
    let db = synced_db();
    let create = fields(&[
        ("title", json!("Original")),
        ("content_type", json!("article")),
        ("status", json!("inbox")),
    ]);
    db.emit_change(
        EntityType::Document,
        "doc-3",
        Op::Upsert,
        create,
        vec![],
        4_000,
    )
    .unwrap();

    let patch = fields(&[("status", json!("archived"))]);
    db.emit_change(
        EntityType::Document,
        "doc-3",
        Op::FieldPatch,
        patch,
        vec![],
        4_100,
    )
    .unwrap();

    let read = db
        .read_entity_fields(EntityType::Document, "doc-3")
        .unwrap()
        .unwrap();
    assert_eq!(read.get("title").unwrap(), &json!("Original"));
    assert_eq!(read.get("status").unwrap(), &json!("archived"));
}

#[test]
fn delete_removes_row_and_records_tombstone() {
    let db = synced_db();
    let f = fields(&[("name", json!("temp"))]);
    db.emit_change(EntityType::Tag, "tag-2", Op::Upsert, f, vec![], 5_000)
        .unwrap();
    assert!(
        db.read_entity_fields(EntityType::Tag, "tag-2")
            .unwrap()
            .is_some()
    );

    db.emit_change(
        EntityType::Tag,
        "tag-2",
        Op::Delete,
        FieldMap::new(),
        vec![],
        5_100,
    )
    .unwrap();
    assert!(
        db.read_entity_fields(EntityType::Tag, "tag-2")
            .unwrap()
            .is_none()
    );
    assert!(db.tombstone(EntityType::Tag, "tag-2").unwrap().is_some());
}

#[test]
fn tag_edge_add_then_remove_toggles_membership() {
    let db = synced_db();
    // Parent rows so the join is meaningful.
    let doc = fields(&[
        ("title", json!("Doc")),
        ("content_type", json!("article")),
        ("status", json!("inbox")),
    ]);
    db.emit_change(EntityType::Document, "d1", Op::Upsert, doc, vec![], 6_000)
        .unwrap();
    let tag = fields(&[("name", json!("fav"))]);
    db.emit_change(EntityType::Tag, "t1", Op::Upsert, tag, vec![], 6_010)
        .unwrap();

    let add = fields(&[("present", json!(true))]);
    db.emit_change(EntityType::TagEdge, "d1:t1", Op::Upsert, add, vec![], 6_100)
        .unwrap();
    assert!(
        db.set_edge(EntityType::TagEdge, "d1:t1")
            .unwrap()
            .is_present()
    );

    let remove = fields(&[("present", json!(false))]);
    db.emit_change(
        EntityType::TagEdge,
        "d1:t1",
        Op::Upsert,
        remove,
        vec![],
        6_200,
    )
    .unwrap();
    assert!(
        !db.set_edge(EntityType::TagEdge, "d1:t1")
            .unwrap()
            .is_present()
    );
}

#[test]
fn applied_guard_is_idempotent() {
    let db = test_db();
    assert!(!db.is_change_applied("chg-1").unwrap());
    db.mark_change_applied("chg-1", 7).unwrap();
    db.mark_change_applied("chg-1", 7).unwrap();
    assert!(db.is_change_applied("chg-1").unwrap());
}

#[test]
fn conflict_inbox_insert_list_dismiss() {
    let db = test_db();
    let clock = Hlc::new(9_000, 1, "device-b".to_owned());
    let id = db
        .insert_conflict(
            EntityType::Document,
            "doc-x",
            "content_text",
            &json!("losing body"),
            &clock,
        )
        .unwrap();
    let open = db.list_conflicts(false).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].entity_id, "doc-x");

    db.dismiss_conflict(&id).unwrap();
    assert_eq!(db.list_conflicts(false).unwrap().len(), 0);
    assert_eq!(db.list_conflicts(true).unwrap().len(), 1);
}

#[test]
fn highlight_round_trips_across_two_tables() {
    let db = synced_db();
    let f = fields(&[
        ("title", json!("Quote source")),
        ("status", json!("inbox")),
        ("quote_text", json!("the quote")),
        ("note", json!("my note")),
        ("color", json!("yellow")),
    ]);
    db.emit_change(EntityType::Highlight, "hl-1", Op::Upsert, f, vec![], 8_000)
        .unwrap();
    let read = db
        .read_entity_fields(EntityType::Highlight, "hl-1")
        .unwrap()
        .unwrap();
    assert_eq!(read.get("quote_text").unwrap(), &json!("the quote"));
    assert_eq!(read.get("note").unwrap(), &json!("my note"));
    assert_eq!(read.get("title").unwrap(), &json!("Quote source"));
}
