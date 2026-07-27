//! Round-trip tests for the optional encrypted backup container (issue #182).
//!
//! These verify that `export_encrypted` → `restore_encrypted` reproduces the
//! same data and [`BackupStats`] as the plaintext `export` → `restore` path,
//! that a wrong passphrase fails cleanly, and that [`is_encrypted_backup`]
//! distinguishes the two container forms.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

use pergamon_core::content_type::ContentType;
use pergamon_core::model::{ContentItem, Feed, Tag};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::backup::{
    ENCRYPTED_MAGIC, export, export_encrypted, restore, restore_encrypted,
};
use pergamon_storage::{BackupStats, Database, is_encrypted_backup};
use time::OffsetDateTime;
use uuid::Uuid;

const PASSPHRASE: &[u8] = b"correct horse battery staple";

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// Build an in-memory database seeded with one feed, item, and tag.
fn seeded_db() -> Database {
    let db = Database::open_in_memory().unwrap_or_else(|e| unreachable!("failed to open DB: {e}"));

    let feed = Feed {
        id: Uuid::new_v4(),
        url: "https://example.com/feed.xml".to_owned(),
        title: "Example Feed".to_owned(),
        site_url: Some("https://example.com".to_owned()),
        description: Some("An example feed".to_owned()),
        folder_id: None,
        last_fetched_at: None,
        created_at: now(),
        updated_at: now(),
        etag: None,
        last_modified_header: None,
        error_count: 0,
        last_error: None,
    };
    db.insert_feed(&feed).unwrap();

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some("https://example.com/post".to_owned()),
        title: "Encrypted Post".to_owned(),
        author: Some("Author".to_owned()),
        content_type: ContentType::Article,
        status: DocumentStatus::Inbox,
        content_text: Some("Post body".to_owned()),
        excerpt: Some("Post excerpt".to_owned()),
        published_at: Some(now()),
        created_at: now(),
        updated_at: now(),
        read_at: None,
    };
    db.insert_content_item(&item).unwrap();

    let tag = Tag {
        id: Uuid::new_v4(),
        name: "encrypted".to_owned(),
        created_at: now(),
    };
    db.insert_tag(&tag).unwrap();
    db.tag_content_item(item.id, tag.id).unwrap();

    db
}

/// Export `db` to a plaintext archive and return its bytes plus the stats.
fn plaintext_bytes(db: &Database) -> (Vec<u8>, BackupStats) {
    let mut buf = Cursor::new(Vec::new());
    let stats = export(db, &mut buf).unwrap();
    (buf.into_inner(), stats)
}

#[test]
fn encrypted_round_trip_matches_plaintext() {
    let src = seeded_db();
    let (_, plaintext_stats) = plaintext_bytes(&src);

    // Encrypt into memory, then restore into a fresh database.
    let mut encrypted = Vec::new();
    let export_stats = export_encrypted(&src, &mut encrypted, PASSPHRASE).unwrap();
    assert_eq!(export_stats, plaintext_stats);

    let dst = Database::open_in_memory().unwrap();
    let restore_stats = restore_encrypted(&dst, Cursor::new(encrypted), PASSPHRASE).unwrap();
    assert_eq!(restore_stats, plaintext_stats);

    // Content is intact and searchable after decryption + restore.
    let items = dst.list_all_content_items().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Encrypted Post");
    assert_eq!(dst.list_feeds().unwrap().len(), 1);
    assert_eq!(dst.list_tags().unwrap().len(), 1);
    assert_eq!(dst.search("Encrypted Post").unwrap().len(), 1);
}

#[test]
fn wrong_passphrase_fails_cleanly() {
    let src = seeded_db();
    let mut encrypted = Vec::new();
    export_encrypted(&src, &mut encrypted, PASSPHRASE).unwrap();

    let dst = Database::open_in_memory().unwrap();
    let err = restore_encrypted(&dst, Cursor::new(encrypted), b"wrong passphrase")
        .expect_err("wrong passphrase must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("wrong passphrase or corrupt archive"),
        "unexpected error message: {msg}"
    );
    // The destination must be untouched by a failed restore.
    assert!(dst.list_all_content_items().unwrap().is_empty());
}

#[test]
fn is_encrypted_backup_distinguishes_forms() {
    let src = seeded_db();

    let (plaintext, _) = plaintext_bytes(&src);
    assert!(!is_encrypted_backup(&plaintext));
    // A plaintext ZIP begins with the local-file-header magic.
    assert_eq!(&plaintext[..2], b"PK");

    let mut encrypted = Vec::new();
    export_encrypted(&src, &mut encrypted, PASSPHRASE).unwrap();
    assert!(is_encrypted_backup(&encrypted));
    assert_eq!(&encrypted[..ENCRYPTED_MAGIC.len()], &ENCRYPTED_MAGIC);

    // A short prefix must not be misclassified.
    assert!(!is_encrypted_backup(b"PGM"));
}

#[test]
fn plaintext_round_trip_still_works() {
    let src = seeded_db();
    let (plaintext, stats) = plaintext_bytes(&src);

    let dst = Database::open_in_memory().unwrap();
    let restored = restore(&dst, Cursor::new(plaintext)).unwrap();
    assert_eq!(restored, stats);
    assert_eq!(dst.list_all_content_items().unwrap().len(), 1);
}

#[test]
fn restore_encrypted_rejects_plaintext_archive() {
    let src = seeded_db();
    let (plaintext, _) = plaintext_bytes(&src);

    let dst = Database::open_in_memory().unwrap();
    let err = restore_encrypted(&dst, Cursor::new(plaintext), PASSPHRASE)
        .expect_err("plaintext must be rejected as not-encrypted");
    assert!(
        format!("{err}").contains("not an encrypted pergamon backup"),
        "unexpected error: {err}"
    );
}
