// SPDX-License-Identifier: Apache-2.0

//! End-to-end proof of the sync baseline backfill (issue #184).
//!
//! The core acceptance criterion: enabling sync on a device that already has a
//! library must enqueue a *complete* baseline so the first push uploads the
//! whole library, and a **fresh** device that pulls reconstructs it exactly —
//! documents and their fields, tag & collection memberships, notes, highlights,
//! and review cards/logs. These tests seed device A while sync is disabled
//! (so nothing lands in the outbox), enable sync, run the baseline, push over a
//! shared in-memory transport, then pull onto a brand-new device B and compare.
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::too_many_lines)]

use pergamon_core::sync::event::{EntityType, Op};
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use pergamon_sync::{CryptoContext, MemoryBlobStore, MemoryTransport, SyncEngine, SyncError};
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

/// A fresh in-memory DB with sync **disabled** (no device identity).
fn fresh_db() -> Database {
    Database::open_in_memory().unwrap()
}

fn enable_sync(db: &Database, device: &str) {
    db.set_sync_identity(&account_hex(), device, 0, Some("mem://test"))
        .unwrap();
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

/// Seed a spread of pre-existing entities on `db` (sync disabled), returning the
/// ids needed for assertions. Emitted in foreign-key-safe order.
struct Seeded {
    feed: String,
    tag1: String,
    tag2: String,
    coll_parent: String,
    coll_child: String,
    doc1: String,
    doc2: String,
    note: String,
    highlight: String,
    card: String,
}

fn seed_library(db: &Database) -> Seeded {
    let feed = "11111111-1111-4111-8111-111111111111".to_owned();
    let tag1 = "22222222-2222-4222-8222-222222222221".to_owned();
    let tag2 = "22222222-2222-4222-8222-222222222222".to_owned();
    let coll_parent = "33333333-3333-4333-8333-333333333331".to_owned();
    let coll_child = "33333333-3333-4333-8333-333333333332".to_owned();
    let doc1 = "44444444-4444-4444-8444-444444444441".to_owned();
    let doc2 = "44444444-4444-4444-8444-444444444442".to_owned();
    let note = "55555555-5555-4555-8555-555555555551".to_owned();
    let highlight = "66666666-6666-4666-8666-666666666661".to_owned();
    let card = "77777777-7777-4777-8777-777777777771".to_owned();
    let log = "88888888-8888-4888-8888-888888888881".to_owned();

    let mut ts = 1_000u64;
    let mut emit = |et: EntityType, id: &str, f: FieldMap| {
        db.emit_change(et, id, Op::Upsert, f, vec![], ts).unwrap();
        ts += 10;
    };

    emit(
        EntityType::FeedSubscription,
        &feed,
        fields(&[
            ("title", json!("My Feed")),
            ("url", json!("https://feed.example/rss")),
            ("site_url", json!("https://feed.example")),
            ("description", json!("a seeded feed")),
        ]),
    );
    emit(EntityType::Tag, &tag1, fields(&[("name", json!("rust"))]));
    emit(EntityType::Tag, &tag2, fields(&[("name", json!("sync"))]));
    emit(
        EntityType::Collection,
        &coll_parent,
        fields(&[("name", json!("Reading")), ("sort_order", json!(0))]),
    );
    emit(
        EntityType::Collection,
        &coll_child,
        fields(&[
            ("name", json!("Deep Dives")),
            ("parent_id", json!(coll_parent)),
            ("sort_order", json!(1)),
        ]),
    );
    emit(
        EntityType::Document,
        &doc1,
        fields(&[
            ("url", json!("https://a.example/1")),
            ("title", json!("Doc One")),
            ("author", json!("Alice")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
            ("content_text", json!("body of doc one")),
            ("excerpt", json!("excerpt one")),
        ]),
    );
    emit(
        EntityType::Document,
        &doc2,
        fields(&[
            ("url", json!("https://a.example/2")),
            ("title", json!("Doc Two")),
            ("content_type", json!("article")),
            ("status", json!("archived")),
            ("content_text", json!("body of doc two")),
        ]),
    );
    // Highlight lives on doc1 (source_item_id FK must resolve, so after docs).
    emit(
        EntityType::Highlight,
        &highlight,
        fields(&[
            ("title", json!("A Highlight")),
            ("status", json!("inbox")),
            ("quote_text", json!("the quoted passage")),
            ("note", json!("why it matters")),
            ("color", json!("yellow")),
            ("position_start", json!(0)),
            ("position_end", json!(18)),
            ("source_item_id", json!(doc1)),
        ]),
    );
    emit(
        EntityType::Note,
        &note,
        fields(&[
            ("content_item_id", json!(doc1)),
            ("body", json!("a standalone note")),
        ]),
    );
    // Review card on the highlight (FK to highlight_meta), then one review log.
    emit(
        EntityType::ReviewCard,
        &card,
        fields(&[
            ("content_item_id", json!(highlight)),
            ("state", json!("review")),
            ("stability", json!(5.0)),
            ("difficulty", json!(3.0)),
            ("due_at", json!("2030-01-01T00:00:00Z")),
            ("review_count", json!(1)),
            ("lapse_count", json!(0)),
            ("scheduled_days", json!(5.0)),
        ]),
    );
    emit(
        EntityType::ReviewLog,
        &log,
        fields(&[
            ("card_id", json!(card)),
            ("rating", json!(3)),
            ("state_before", json!("new")),
            ("state_after", json!("review")),
            ("stability_after", json!(5.0)),
            ("difficulty_after", json!(3.0)),
            ("elapsed_days", json!(0.0)),
            ("scheduled_days", json!(5.0)),
            ("reviewed_at", json!("2025-01-01T00:00:00Z")),
        ]),
    );
    // Tag & collection membership edges.
    emit(
        EntityType::TagEdge,
        &format!("{doc1}:{tag1}"),
        fields(&[("present", json!(true))]),
    );
    emit(
        EntityType::TagEdge,
        &format!("{doc1}:{tag2}"),
        fields(&[("present", json!(true))]),
    );
    emit(
        EntityType::TagEdge,
        &format!("{doc2}:{tag1}"),
        fields(&[("present", json!(true))]),
    );
    emit(
        EntityType::CollectionEdge,
        &format!("{doc1}:{coll_parent}"),
        fields(&[("present", json!(true))]),
    );
    emit(
        EntityType::CollectionEdge,
        &format!("{doc2}:{coll_child}"),
        fields(&[("present", json!(true))]),
    );

    // Derive the card from its log so device A's stored card equals what any
    // peer computes from the same log union (apply recomputes on both sides).
    db.recompute_review_card(&card).unwrap();

    Seeded {
        feed,
        tag1,
        tag2,
        coll_parent,
        coll_child,
        doc1,
        doc2,
        note,
        highlight,
        card,
    }
}

/// The heart of issue #184: a fresh device reconstructs the full library from
/// the baseline that enabling sync enqueued.
#[test]
fn baseline_lets_fresh_device_reconstruct_full_library() {
    let transport = MemoryTransport::new();
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());

    // Device A: seed BEFORE enabling sync, so nothing pre-exists in the outbox.
    let db_a = fresh_db();
    let s = seed_library(&db_a);
    assert_eq!(
        db_a.pending_outbox_count().unwrap(),
        0,
        "seeding with sync disabled must not enqueue anything"
    );

    // Enable sync and backfill the baseline.
    enable_sync(&db_a, "device-a");
    let enqueued = db_a.enqueue_sync_baseline(2_000).unwrap();
    assert_eq!(
        enqueued, 16,
        "baseline must enqueue one change per seeded entity + edge"
    );
    assert_eq!(db_a.pending_outbox_count().unwrap(), 16);

    // Before pushing, the completeness check must fail loudly.
    let eng_a = engine(&transport, "device-a");
    match eng_a.verify_upload_complete(&db_a) {
        Err(SyncError::IncompleteUpload {
            pending_events,
            missing_blobs,
        }) => {
            assert_eq!(pending_events, 16);
            assert!(missing_blobs.is_empty());
        }
        other => panic!("expected IncompleteUpload before push, got {other:?}"),
    }

    // Push, then the completeness check passes.
    let pushed = eng_a.push(&db_a, &blob_a).unwrap();
    assert_eq!(pushed, 16);
    eng_a.verify_upload_complete(&db_a).unwrap();
    assert_eq!(db_a.pending_outbox_count().unwrap(), 0);

    // Re-running the baseline is a no-op (run-once guard).
    assert_eq!(db_a.enqueue_sync_baseline(3_000).unwrap(), 0);
    assert_eq!(db_a.pending_outbox_count().unwrap(), 0);

    // Device B: brand-new DB, same account/crypto, different device id. Pull.
    let db_b = fresh_db();
    enable_sync(&db_b, "device-b");
    let eng_b = engine(&transport, "device-b");
    let applied = eng_b.pull(&db_b, &blob_b).unwrap();
    assert_eq!(applied, 16, "B applies every baseline change");

    // --- Full-library reconstruction: A and B must agree on every entity. ---
    for (et, id) in [
        (EntityType::FeedSubscription, &s.feed),
        (EntityType::Tag, &s.tag1),
        (EntityType::Tag, &s.tag2),
        (EntityType::Collection, &s.coll_parent),
        (EntityType::Collection, &s.coll_child),
        (EntityType::Document, &s.doc1),
        (EntityType::Document, &s.doc2),
        (EntityType::Note, &s.note),
        (EntityType::Highlight, &s.highlight),
        (EntityType::ReviewCard, &s.card),
    ] {
        let a = read(&db_a, et, id);
        let b = read(&db_b, et, id);
        assert!(b.is_some(), "{et:?} {id} missing on fresh device B");
        assert_eq!(a, b, "{et:?} {id} differs between A and B");
    }

    // Spot-check reconstructed content, not just equality.
    let doc1_b = read(&db_b, EntityType::Document, &s.doc1).unwrap();
    assert_eq!(doc1_b.get("title").unwrap(), &json!("Doc One"));
    assert_eq!(
        doc1_b.get("content_text").unwrap(),
        &json!("body of doc one")
    );
    assert_eq!(doc1_b.get("author").unwrap(), &json!("Alice"));

    let hl_b = read(&db_b, EntityType::Highlight, &s.highlight).unwrap();
    assert_eq!(
        hl_b.get("quote_text").unwrap(),
        &json!("the quoted passage")
    );
    assert_eq!(hl_b.get("source_item_id").unwrap(), &json!(s.doc1));

    // Tag memberships: doc1 has both tags, doc2 has only tag1.
    assert!(edge_present(&db_b, EntityType::TagEdge, &s.doc1, &s.tag1));
    assert!(edge_present(&db_b, EntityType::TagEdge, &s.doc1, &s.tag2));
    assert!(edge_present(&db_b, EntityType::TagEdge, &s.doc2, &s.tag1));
    assert!(!edge_present(&db_b, EntityType::TagEdge, &s.doc2, &s.tag2));

    // Collection memberships: doc1 in parent, doc2 in child.
    assert!(edge_present(
        &db_b,
        EntityType::CollectionEdge,
        &s.doc1,
        &s.coll_parent
    ));
    assert!(edge_present(
        &db_b,
        EntityType::CollectionEdge,
        &s.doc2,
        &s.coll_child
    ));

    // Review log reconstructed and attached to the card.
    let card_uuid = uuid_of(&s.card);
    assert_eq!(
        db_b.list_review_logs_for_card(card_uuid).unwrap().len(),
        1,
        "the review log must replicate to B"
    );
}

fn edge_present(db: &Database, et: EntityType, left: &str, right: &str) -> bool {
    db.set_edge(et, &format!("{left}:{right}"))
        .unwrap()
        .is_present()
}

fn uuid_of(id: &str) -> uuid::Uuid {
    uuid::Uuid::parse_str(id).unwrap()
}

/// Enabling sync on an empty library enqueues zero baseline changes and does not
/// error.
#[test]
fn baseline_on_empty_library_is_zero() {
    let db = fresh_db();
    enable_sync(&db, "device-a");
    assert_eq!(db.enqueue_sync_baseline(1_000).unwrap(), 0);
    assert_eq!(db.pending_outbox_count().unwrap(), 0);
    // Idempotent even when empty.
    assert_eq!(db.enqueue_sync_baseline(2_000).unwrap(), 0);
}

/// The baseline refuses to run before sync identity exists.
#[test]
fn baseline_requires_sync_enabled() {
    let db = fresh_db();
    assert!(db.enqueue_sync_baseline(1_000).is_err());
}
