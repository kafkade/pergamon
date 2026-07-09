// SPDX-License-Identifier: AGPL-3.0-only

//! Sync change-tracking for server-side mutations (issue #129).
//!
//! When remote sync is enabled (a device identity is persisted), local writes
//! must be recorded in the outbox so the background sync worker can push them to
//! other devices. The JSON/HTML routes mutate the canonical store directly; this
//! module mirrors the CLI's `track_document_upsert` so an edit made through the
//! web UI propagates just like a CLI edit.
//!
//! Every helper is **best-effort**: a tracking failure only logs and never fails
//! the user's request, and all are no-ops when sync is disabled.

use pergamon_core::model::ContentItem;
use pergamon_core::sync::event::{EntityType, Op};
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use time::OffsetDateTime;

/// Whether remote sync is enabled for this database (a device identity exists).
fn sync_enabled(db: &Database) -> bool {
    db.sync_state().is_ok_and(|s| s.device_id.is_some())
}

/// Current wall-clock time in Unix milliseconds, for HLC stamping.
fn now_millis() -> u64 {
    u64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

/// Record a document create/update in the sync outbox. No-op when sync is
/// disabled; logs and swallows any tracking error.
pub fn track_document_upsert(db: &Database, item: &ContentItem) {
    if !sync_enabled(db) {
        return;
    }
    let mut fields = FieldMap::new();
    fields.insert("title".to_owned(), serde_json::json!(item.title));
    fields.insert(
        "content_type".to_owned(),
        serde_json::json!(item.content_type.as_str()),
    );
    fields.insert("status".to_owned(), serde_json::json!(item.status.as_str()));
    if let Some(url) = &item.url {
        fields.insert("url".to_owned(), serde_json::json!(url));
    }
    if let Some(author) = &item.author {
        fields.insert("author".to_owned(), serde_json::json!(author));
    }
    if let Some(text) = &item.content_text {
        fields.insert("content_text".to_owned(), serde_json::json!(text));
    }
    if let Some(excerpt) = &item.excerpt {
        fields.insert("excerpt".to_owned(), serde_json::json!(excerpt));
    }
    if let Err(e) = db.emit_change(
        EntityType::Document,
        &item.id.to_string(),
        Op::Upsert,
        fields,
        Vec::new(),
        now_millis(),
    ) {
        tracing::warn!("failed to track document upsert for sync: {e}");
    }
}

/// Re-fetch a document by id and record it as an upsert. Convenience for routes
/// that mutate status/tags and hold only the id. No-op when sync is disabled.
pub fn track_document_by_id(db: &Database, id: uuid::Uuid) {
    if !sync_enabled(db) {
        return;
    }
    match db.get_content_item(id) {
        Ok(item) => track_document_upsert(db, &item),
        Err(e) => tracing::warn!("failed to load document {id} for sync tracking: {e}"),
    }
}

/// Record a document deletion in the sync outbox. No-op when sync is disabled;
/// logs and swallows any tracking error.
pub fn track_document_delete(db: &Database, id: uuid::Uuid) {
    if !sync_enabled(db) {
        return;
    }
    if let Err(e) = db.emit_change(
        EntityType::Document,
        &id.to_string(),
        Op::Delete,
        FieldMap::new(),
        Vec::new(),
        now_millis(),
    ) {
        tracing::warn!("failed to track document delete for sync: {e}");
    }
}
