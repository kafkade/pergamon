// SPDX-License-Identifier: Apache-2.0

//! Acceptance test for issue #128: a brand-new device, onboarded through the
//! full enrollment flow, restores a large library and its review state without
//! corrupting identity, rules, or due counts.
//!
//! This is the end-to-end proof of the acceptance criterion. It wires two
//! independent in-memory doubles together:
//!
//! - a [`MemoryRelay`] plays the onboarding relay (device records, sealed
//!   enrollment bundles, trust attestations), and
//! - a [`MemoryTransport`] plays the change-sync "server" (encrypted events).
//!
//! Device A bootstraps the account, fills a library plus review cards and logs,
//! and pushes. Device B — which never sees the Account Root Key up front —
//! derives it purely by enrolling, being approved by A, and accepting. It then
//! decrypts and pulls A's pushed history, and we assert the two databases agree
//! on documents, highlights, review cards, review logs, and the due-card count.
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use pergamon_core::sync::event::{EntityType, Op};
use pergamon_crypto::device::DeviceKeypairs;
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use pergamon_sync::{CryptoContext, MemoryBlobStore, MemoryRelay, MemoryTransport, SyncEngine};
use serde_json::{Value, json};

/// Size of the "large library" the new device must restore.
const LIBRARY_SIZE: usize = 200;
/// Review cards whose index is a multiple of this are scheduled as due now.
const DUE_EVERY: usize = 3;
/// A due timestamp in the past (card is due) and one far in the future (not due).
const DUE_NOW: &str = "2023-01-01T00:00:00Z";
const DUE_LATER: &str = "2099-01-01T00:00:00Z";
/// The "now" cutoff for counting due cards.
const DUE_CUTOFF: &str = "2024-01-01T00:00:00Z";

fn fields(pairs: &[(&str, Value)]) -> FieldMap {
    let mut m = FieldMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    m
}

/// Count review cards whose `due_at` is at or before the cutoff. RFC3339
/// timestamps in UTC sort chronologically as plain strings.
fn due_count(db: &Database) -> usize {
    (0..LIBRARY_SIZE)
        .filter(|i| {
            db.read_entity_fields(EntityType::ReviewCard, &format!("card-{i}"))
                .unwrap()
                .and_then(|f| {
                    f.get("due_at")
                        .and_then(Value::as_str)
                        .map(|s| s <= DUE_CUTOFF)
                })
                .unwrap_or(false)
        })
        .count()
}

/// Populate `db` with a large library plus highlights, review cards, and logs.
///
/// Emits in foreign-key order (document/highlight shell before its review card,
/// review card before its log) so the local apply during `emit_change` and the
/// remote apply during `pull` both satisfy the schema's referential integrity.
fn seed_library(db: &Database) {
    for i in 0..LIBRARY_SIZE {
        let clock = 1_000 + i as u64;

        db.emit_change(
            EntityType::Document,
            &format!("doc-{i}"),
            Op::Upsert,
            fields(&[
                ("title", json!(format!("Article {i}"))),
                ("content_type", json!("article")),
                (
                    "status",
                    json!(if i % 2 == 0 { "inbox" } else { "archived" }),
                ),
                ("content_text", json!(format!("body text for article {i}"))),
            ]),
            vec![],
            clock,
        )
        .unwrap();

        // A highlight is the content the review card hangs off of.
        db.emit_change(
            EntityType::Highlight,
            &format!("hl-{i}"),
            Op::Upsert,
            fields(&[
                ("title", json!(format!("Highlight {i}"))),
                ("status", json!("inbox")),
                ("quote_text", json!(format!("memorable quote {i}"))),
                ("color", json!("yellow")),
            ]),
            vec![],
            clock,
        )
        .unwrap();

        // The review card. Cards that will get a review log below are seeded
        // due-now; the rest are parked far in the future. A card's schedule is
        // *derived* from its logs on write, so only logged cards get a computed
        // near-term due date — the others keep their seeded `due_at`.
        let will_have_log = i % DUE_EVERY == 0;
        let due_at = if will_have_log { DUE_NOW } else { DUE_LATER };
        db.emit_change(
            EntityType::ReviewCard,
            &format!("card-{i}"),
            Op::Upsert,
            fields(&[
                ("content_item_id", json!(format!("hl-{i}"))),
                ("state", json!("review")),
                ("due_at", json!(due_at)),
                ("review_count", json!(i % 5)),
                ("lapse_count", json!(i % 2)),
            ]),
            vec![],
            clock,
        )
        .unwrap();

        if !will_have_log {
            continue;
        }
        // An append-only review log row for the card.
        db.emit_change(
            EntityType::ReviewLog,
            &format!("log-{i}"),
            Op::Upsert,
            fields(&[
                ("card_id", json!(format!("card-{i}"))),
                ("rating", json!((i % 4) + 1)),
                ("state_before", json!("new")),
                ("state_after", json!("review")),
                ("stability_after", json!(1.0)),
                ("difficulty_after", json!(5.0)),
                ("elapsed_days", json!(0.0)),
                ("scheduled_days", json!(1.0)),
                ("reviewed_at", json!(DUE_NOW)),
            ]),
            vec![],
            clock,
        )
        .unwrap();
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn new_device_onboards_and_restores_library_and_review_state() {
    const NOW: i64 = 1_700_000_000_000;

    // Two independent in-memory "servers": one for onboarding, one for changes.
    let relay = MemoryRelay::new();
    let transport = MemoryTransport::new();

    // --- Device A: the account's founding device. --------------------------
    let ark = AccountRootKey::generate().unwrap();
    let account_id = AccountId::generate().unwrap();
    let account_hex = account_id.to_hex();
    let dev_a = DeviceKeypairs::generate().unwrap();

    // A bootstraps the account onto the onboarding relay.
    pergamon_sync::onboarding::bootstrap(&relay, &account_id, &dev_a, 0, NOW).unwrap();

    // A builds its change-sync engine and fills a large library.
    let db_a = Database::open_in_memory().unwrap();
    db_a.set_sync_identity(&account_hex, dev_a.device_id(), 0, Some("mem://test"))
        .unwrap();
    let eng_a = SyncEngine::new(
        transport.clone(),
        CryptoContext::new(
            AccountRootKey::from_bytes(*ark.expose_bytes()),
            account_hex,
            dev_a.device_id().to_owned(),
            0,
        )
        .unwrap(),
    );
    let blob_a = MemoryBlobStore::new();
    seed_library(&db_a);
    // The review flow recomputes a card's FSRS schedule from its logs on write;
    // the raw emit_change seeding above skips that, so mirror it explicitly here.
    // The pulling device recomputes the same way, so both converge on the same
    // derived schedule.
    for i in 0..LIBRARY_SIZE {
        db_a.recompute_review_card(&format!("card-{i}")).unwrap();
    }
    let pushed = eng_a.push(&db_a, &blob_a).unwrap();
    assert!(
        pushed >= LIBRARY_SIZE * 3,
        "A should push the whole library ({pushed} changes)"
    );

    // --- Device B: a brand-new device that knows only the invite. ----------
    // It learns the opaque account handle out-of-band (the invite), but never
    // the Account Root Key.
    let dev_b = DeviceKeypairs::generate().unwrap();
    pergamon_sync::onboarding::enroll_publish(&relay, &account_id, &dev_b, NOW).unwrap();

    // Both devices independently compute a matching SAS.
    let sas_a =
        pergamon_sync::onboarding::sas_against(&relay, &account_id, &dev_a, dev_b.device_id())
            .unwrap();
    let sas_b =
        pergamon_sync::onboarding::sas_against(&relay, &account_id, &dev_b, dev_a.device_id())
            .unwrap();
    assert!(sas_a.matches(&sas_b), "SAS must match out-of-band");

    // A approves B; B accepts and derives the ARK purely from the flow.
    pergamon_sync::onboarding::approve(
        &relay,
        &account_id,
        &dev_a,
        &ark,
        0,
        dev_b.device_id(),
        NOW,
    )
    .unwrap();
    let accepted = pergamon_sync::onboarding::accept(&relay, &account_id, &dev_b).unwrap();
    assert_eq!(
        accepted.bundle.ark.expose_bytes(),
        ark.expose_bytes(),
        "the onboarded device must recover A's exact account key"
    );
    assert_eq!(accepted.bundle.account_id, account_id);
    assert_eq!(
        accepted.approver_device_id.as_deref(),
        Some(dev_a.device_id())
    );

    // --- Device B restores the library over the change-sync transport. -----
    let db_b = Database::open_in_memory().unwrap();
    db_b.set_sync_identity(
        &accepted.bundle.account_id.to_hex(),
        dev_b.device_id(),
        accepted.bundle.key_epoch,
        Some("mem://test"),
    )
    .unwrap();
    let eng_b = SyncEngine::new(
        transport,
        CryptoContext::new(
            accepted.bundle.ark,
            accepted.bundle.account_id.to_hex(),
            dev_b.device_id().to_owned(),
            accepted.bundle.key_epoch,
        )
        .unwrap(),
    );
    let blob_b = MemoryBlobStore::new();
    let applied = eng_b.pull(&db_b, &blob_b).unwrap();
    assert!(applied >= pushed, "B must apply everything A pushed");

    // --- Assert identity, library, and review state are intact. ------------
    for i in 0..LIBRARY_SIZE {
        let mut entities = vec![
            (EntityType::Document, format!("doc-{i}")),
            (EntityType::Highlight, format!("hl-{i}")),
            (EntityType::ReviewCard, format!("card-{i}")),
        ];
        // Only every DUE_EVERY-th card has a review log (see seed_library).
        if i % DUE_EVERY == 0 {
            entities.push((EntityType::ReviewLog, format!("log-{i}")));
        }
        for (et, id) in entities {
            let a = db_a.read_entity_fields(et, &id).unwrap();
            let b = db_b.read_entity_fields(et, &id).unwrap();
            assert!(b.is_some(), "{et:?} {id} must exist on the restored device");
            assert_eq!(a, b, "{et:?} {id} must round-trip intact");
        }
    }

    // Due counts — the acceptance criterion's headline number — must agree and
    // be non-trivial. Exactly the logged cards (every DUE_EVERY-th) are due.
    let expected_due = LIBRARY_SIZE.div_ceil(DUE_EVERY);
    assert_eq!(due_count(&db_a), expected_due, "A's due count");
    assert_eq!(
        due_count(&db_b),
        due_count(&db_a),
        "due-card counts must match after restore"
    );
}
