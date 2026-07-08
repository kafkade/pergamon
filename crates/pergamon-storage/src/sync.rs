//! Client sync-engine persistence (ADR-022 / ADR-023, issue #126).
//!
//! This module is the storage half of the sync engine: it owns the outbox,
//! per-field hybrid-logical-clock tracking, observed-remove set-edge state,
//! delete tombstones, the applied-change idempotency guard, and the conflict
//! inbox — all in the tables introduced by migration V13. It also provides the
//! **canonical field read/write** helpers that translate between an ADR-022
//! event body's generic `field -> value` map and the typed entity tables, so
//! the pure merge policy in `pergamon-core::sync` can be applied uniformly.
//!
//! Two entry points bracket a change's life:
//!
//! - [`Database::emit_change`] — a *local* mutation writes its canonical
//!   row(s), stamps per-field clocks, and enqueues an outbox row, atomically.
//! - the read/write/clock primitives below — the engine (`pergamon-sync`)
//!   resolves a *pulled* change through the core merge policy and applies the
//!   result via these, recording clocks, tombstones, and conflict copies.

use rusqlite::{OptionalExtension, params};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use pergamon_core::fsrs::{Rating, Scheduler};
use pergamon_core::sync::event::{BlobManifestEntry, ChangeBody, EntityType, Op};
use pergamon_core::sync::hlc::Hlc;
use pergamon_core::sync::merge::{ConflictStrategy, SetMember, strategy_for};
use pergamon_core::sync::review::{DerivedCard, ReviewLogEntry, derive_card_state};

use crate::db::Database;
use crate::error::StorageError;

/// A generic entity field map: wire/column field name to JSON value. Mirrors the
/// `fields` map of an ADR-022 [`ChangeBody`].
pub type FieldMap = Map<String, Value>;

/// The persisted engine state: identity, pull cursor, and last local clock.
#[derive(Debug, Clone, Default)]
pub struct SyncState {
    /// Opaque ADR-022 account handle (hex), or `None` until sync is enabled.
    pub account_id: Option<String>,
    /// This device's opaque handle, or `None` until sync is enabled.
    pub device_id: Option<String>,
    /// Account key epoch encrypting new events.
    pub key_epoch: u32,
    /// Greatest server sequence durably applied on pull.
    pub cursor_seq: u64,
    /// Last local HLC wall time (persisted so the clock survives restarts).
    pub hlc_wall_millis: u64,
    /// Last local HLC counter.
    pub hlc_counter: u32,
    /// Configured sync server base URL, if any.
    pub server_url: Option<String>,
}

/// A pending outbox row awaiting push.
#[derive(Debug, Clone)]
pub struct OutboxRow {
    /// Client idempotency key / envelope `change_id`.
    pub change_id: String,
    /// Target entity class.
    pub entity_type: EntityType,
    /// Target entity id.
    pub entity_id: String,
    /// Mutation kind.
    pub op: Op,
    /// The serialized plaintext [`ChangeBody`].
    pub body: Vec<u8>,
    /// Ciphertext hashes of referenced blobs.
    pub blob_refs: Vec<String>,
    /// Local monotonic ordering key.
    pub local_seq: u64,
}

/// A conflict-inbox entry: the losing value of a conflict-copy merge.
#[derive(Debug, Clone)]
pub struct ConflictRow {
    /// Stable id of this conflict entry.
    pub id: String,
    /// Entity class the conflict is on.
    pub entity_type: String,
    /// Entity id the conflict is on.
    pub entity_id: String,
    /// Field whose authored prose diverged.
    pub field: String,
    /// The preserved losing value (JSON-encoded).
    pub loser_value: String,
    /// Clock of the losing value.
    pub loser_clock: Hlc,
    /// When the conflict was recorded (RFC3339).
    pub created_at: String,
    /// Whether the user has dismissed it.
    pub dismissed: bool,
}

#[allow(clippy::missing_errors_doc)]
impl Database {
    // ==================================================================
    // Engine state
    // ==================================================================

    /// Read the single-row sync state.
    pub fn sync_state(&self) -> Result<SyncState, StorageError> {
        let state = self.connection().query_row(
            "SELECT account_id, device_id, key_epoch, cursor_seq,
                    hlc_wall_millis, hlc_counter, server_url
             FROM sync_state WHERE id = 1",
            [],
            |row| {
                Ok(SyncState {
                    account_id: row.get(0)?,
                    device_id: row.get(1)?,
                    key_epoch: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                    cursor_seq: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                    hlc_wall_millis: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                    hlc_counter: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
                    server_url: row.get(6)?,
                })
            },
        )?;
        Ok(state)
    }

    /// Set the account/device identity, key epoch, and server URL.
    pub fn set_sync_identity(
        &self,
        account_id: &str,
        device_id: &str,
        key_epoch: u32,
        server_url: Option<&str>,
    ) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE sync_state
             SET account_id = ?1, device_id = ?2, key_epoch = ?3, server_url = ?4
             WHERE id = 1",
            params![account_id, device_id, i64::from(key_epoch), server_url],
        )?;
        Ok(())
    }

    /// The pull cursor (greatest applied server sequence).
    pub fn sync_cursor(&self) -> Result<u64, StorageError> {
        Ok(self.sync_state()?.cursor_seq)
    }

    /// Advance the account key epoch in place (e.g. after a device revocation),
    /// without disturbing the account/device identity or server URL.
    pub fn set_key_epoch(&self, key_epoch: u32) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE sync_state SET key_epoch = ?1 WHERE id = 1",
            params![i64::from(key_epoch)],
        )?;
        Ok(())
    }

    /// Persist the pull cursor after durably applying a page.
    pub fn set_sync_cursor(&self, seq: u64) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE sync_state SET cursor_seq = ?1 WHERE id = 1",
            params![i64::try_from(seq).unwrap_or(i64::MAX)],
        )?;
        Ok(())
    }

    /// The current local hybrid logical clock (device id from identity).
    pub fn sync_hlc(&self) -> Result<Hlc, StorageError> {
        let s = self.sync_state()?;
        Ok(Hlc::new(
            s.hlc_wall_millis,
            s.hlc_counter,
            s.device_id.unwrap_or_default(),
        ))
    }

    /// Persist the local clock's causal component.
    pub fn set_sync_hlc(&self, clock: &Hlc) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE sync_state SET hlc_wall_millis = ?1, hlc_counter = ?2 WHERE id = 1",
            params![
                i64::try_from(clock.wall_millis).unwrap_or(i64::MAX),
                i64::from(clock.counter),
            ],
        )?;
        Ok(())
    }

    /// Advance the local clock for a new local event and persist it, returning
    /// the stamp to attach to that event.
    pub fn tick_local_hlc(&self, now_millis: u64) -> Result<Hlc, StorageError> {
        let next = self.sync_hlc()?.tick(now_millis);
        self.set_sync_hlc(&next)?;
        Ok(next)
    }

    /// Advance the local clock on observing a remote stamp and persist it.
    pub fn observe_remote_hlc(&self, remote: &Hlc, now_millis: u64) -> Result<(), StorageError> {
        let updated = self.sync_hlc()?.observe(remote, now_millis);
        self.set_sync_hlc(&updated)
    }

    // ==================================================================
    // Outbox
    // ==================================================================

    /// Allocate the next local ordering sequence.
    fn next_local_seq(&self) -> Result<u64, StorageError> {
        let seq: i64 = self.connection().query_row(
            "UPDATE sync_local_seq SET next = next + 1 WHERE id = 1 RETURNING next - 1",
            [],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(seq).unwrap_or(0))
    }

    /// Enqueue an outbox row for `body`, returning its generated `change_id`.
    pub fn enqueue_outbox(&self, body: &ChangeBody) -> Result<String, StorageError> {
        let change_id = Uuid::new_v4().to_string();
        let bytes = body
            .to_bytes()
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let blob_refs = serde_json::to_string(&body.blob_refs())
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let local_seq = self.next_local_seq()?;
        self.connection().execute(
            "INSERT INTO sync_outbox
                (change_id, entity_type, entity_id, op, body, blob_refs, local_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                change_id,
                body.entity_type.as_str(),
                body.entity_id,
                body.op.as_str(),
                bytes,
                blob_refs,
                i64::try_from(local_seq).unwrap_or(i64::MAX),
            ],
        )?;
        Ok(change_id)
    }

    /// The pending (unacknowledged) outbox rows, in local order, up to `limit`.
    pub fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxRow>, StorageError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT change_id, entity_type, entity_id, op, body, blob_refs, local_seq
             FROM sync_outbox WHERE acked_seq IS NULL
             ORDER BY local_seq ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            let entity_type: String = row.get(1)?;
            let op: String = row.get(3)?;
            let blob_refs: String = row.get(5)?;
            Ok((
                row.get::<_, String>(0)?,
                entity_type,
                row.get::<_, String>(2)?,
                op,
                row.get::<_, Vec<u8>>(4)?,
                blob_refs,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (change_id, et, entity_id, op, body, blob_refs, local_seq) = r?;
            out.push(OutboxRow {
                change_id,
                entity_type: EntityType::from_wire(&et)
                    .ok_or_else(|| StorageError::Serialization(format!("bad entity_type {et}")))?,
                entity_id,
                op: Op::from_wire(&op)
                    .ok_or_else(|| StorageError::Serialization(format!("bad op {op}")))?,
                body,
                blob_refs: serde_json::from_str(&blob_refs)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?,
                local_seq: u64::try_from(local_seq).unwrap_or(0),
            });
        }
        Ok(out)
    }

    /// Count pending outbox rows.
    pub fn pending_outbox_count(&self) -> Result<u64, StorageError> {
        let n: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM sync_outbox WHERE acked_seq IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Mark an outbox row acknowledged with the server sequence it was assigned.
    pub fn mark_outbox_acked(&self, change_id: &str, server_seq: u64) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE sync_outbox SET acked_seq = ?2 WHERE change_id = ?1",
            params![change_id, i64::try_from(server_seq).unwrap_or(i64::MAX)],
        )?;
        Ok(())
    }

    // ==================================================================
    // Per-field clocks
    // ==================================================================

    /// The HLC that last wrote `field` of the entity, if tracked.
    pub fn entity_clock(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
    ) -> Result<Option<Hlc>, StorageError> {
        let clock = self
            .connection()
            .query_row(
                "SELECT wall_millis, counter, device_id FROM sync_entity_clock
                 WHERE entity_type = ?1 AND entity_id = ?2 AND field = ?3",
                params![entity_type.as_str(), entity_id, field],
                |row| {
                    Ok(Hlc::new(
                        u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                        u32::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(clock)
    }

    /// Stamp `field` of the entity with `clock` (upsert).
    pub fn set_entity_clock(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
        clock: &Hlc,
    ) -> Result<(), StorageError> {
        self.connection().execute(
            "INSERT INTO sync_entity_clock
                (entity_type, entity_id, field, wall_millis, counter, device_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(entity_type, entity_id, field) DO UPDATE SET
                wall_millis = excluded.wall_millis,
                counter = excluded.counter,
                device_id = excluded.device_id",
            params![
                entity_type.as_str(),
                entity_id,
                field,
                i64::try_from(clock.wall_millis).unwrap_or(i64::MAX),
                i64::from(clock.counter),
                clock.device_id,
            ],
        )?;
        Ok(())
    }

    // ==================================================================
    // Observed-remove set edges (tag / collection membership)
    // ==================================================================

    /// The observed-remove state of a membership edge.
    pub fn set_edge(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<SetMember, StorageError> {
        let member = self
            .connection()
            .query_row(
                "SELECT add_wall, add_counter, add_device, rem_wall, rem_counter, rem_device
                 FROM sync_set_edge WHERE entity_type = ?1 AND entity_id = ?2",
                params![entity_type.as_str(), entity_id],
                |row| {
                    let add = optional_clock(
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    );
                    let rem = optional_clock(
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    );
                    Ok(SetMember {
                        add_clock: add,
                        remove_clock: rem,
                    })
                },
            )
            .optional()?;
        Ok(member.unwrap_or_default())
    }

    /// Persist the observed-remove state of a membership edge.
    pub fn save_set_edge(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        member: &SetMember,
    ) -> Result<(), StorageError> {
        let (aw, ac, ad) = split_clock(member.add_clock.as_ref());
        let (rw, rc, rd) = split_clock(member.remove_clock.as_ref());
        self.connection().execute(
            "INSERT INTO sync_set_edge
                (entity_type, entity_id, add_wall, add_counter, add_device,
                 rem_wall, rem_counter, rem_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(entity_type, entity_id) DO UPDATE SET
                add_wall = excluded.add_wall, add_counter = excluded.add_counter,
                add_device = excluded.add_device, rem_wall = excluded.rem_wall,
                rem_counter = excluded.rem_counter, rem_device = excluded.rem_device",
            params![entity_type.as_str(), entity_id, aw, ac, ad, rw, rc, rd],
        )?;
        Ok(())
    }

    // ==================================================================
    // Delete tombstones
    // ==================================================================

    /// The winning delete clock for an entity, if it has been tombstoned.
    pub fn tombstone(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<Option<Hlc>, StorageError> {
        let clock = self
            .connection()
            .query_row(
                "SELECT wall_millis, counter, device_id FROM sync_tombstones
                 WHERE entity_type = ?1 AND entity_id = ?2",
                params![entity_type.as_str(), entity_id],
                |row| {
                    Ok(Hlc::new(
                        u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                        u32::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(clock)
    }

    /// Record (or advance) a delete tombstone for an entity.
    pub fn set_tombstone(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        clock: &Hlc,
    ) -> Result<(), StorageError> {
        self.connection().execute(
            "INSERT INTO sync_tombstones
                (entity_type, entity_id, wall_millis, counter, device_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(entity_type, entity_id) DO UPDATE SET
                wall_millis = excluded.wall_millis, counter = excluded.counter,
                device_id = excluded.device_id",
            params![
                entity_type.as_str(),
                entity_id,
                i64::try_from(clock.wall_millis).unwrap_or(i64::MAX),
                i64::from(clock.counter),
                clock.device_id,
            ],
        )?;
        Ok(())
    }

    // ==================================================================
    // Applied-change idempotency guard
    // ==================================================================

    /// Whether a pulled `change_id` has already been applied.
    pub fn is_change_applied(&self, change_id: &str) -> Result<bool, StorageError> {
        let n: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM sync_applied WHERE change_id = ?1",
            params![change_id],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    /// Record a pulled `change_id` as applied (idempotent).
    pub fn mark_change_applied(
        &self,
        change_id: &str,
        server_seq: u64,
    ) -> Result<(), StorageError> {
        self.connection().execute(
            "INSERT OR IGNORE INTO sync_applied (change_id, server_seq) VALUES (?1, ?2)",
            params![change_id, i64::try_from(server_seq).unwrap_or(i64::MAX)],
        )?;
        Ok(())
    }

    // ==================================================================
    // Conflict inbox
    // ==================================================================

    /// Record a conflict-copy loser in the conflict inbox.
    pub fn insert_conflict(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
        loser_value: &Value,
        loser_clock: &Hlc,
    ) -> Result<String, StorageError> {
        let id = Uuid::new_v4().to_string();
        self.connection().execute(
            "INSERT INTO sync_conflicts
                (id, entity_type, entity_id, field, loser_value,
                 loser_wall, loser_counter, loser_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                entity_type.as_str(),
                entity_id,
                field,
                loser_value.to_string(),
                i64::try_from(loser_clock.wall_millis).unwrap_or(i64::MAX),
                i64::from(loser_clock.counter),
                loser_clock.device_id,
            ],
        )?;
        Ok(id)
    }

    /// List conflict-inbox entries; set `include_dismissed` to include resolved ones.
    pub fn list_conflicts(
        &self,
        include_dismissed: bool,
    ) -> Result<Vec<ConflictRow>, StorageError> {
        let conn = self.connection();
        let sql = if include_dismissed {
            "SELECT id, entity_type, entity_id, field, loser_value,
                    loser_wall, loser_counter, loser_device, created_at, dismissed
             FROM sync_conflicts ORDER BY created_at DESC"
        } else {
            "SELECT id, entity_type, entity_id, field, loser_value,
                    loser_wall, loser_counter, loser_device, created_at, dismissed
             FROM sync_conflicts WHERE dismissed = 0 ORDER BY created_at DESC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(ConflictRow {
                id: row.get(0)?,
                entity_type: row.get(1)?,
                entity_id: row.get(2)?,
                field: row.get(3)?,
                loser_value: row.get(4)?,
                loser_clock: Hlc::new(
                    u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
                    u32::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
                    row.get::<_, String>(7)?,
                ),
                created_at: row.get(8)?,
                dismissed: row.get::<_, i64>(9)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Mark a conflict entry dismissed.
    pub fn dismiss_conflict(&self, id: &str) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE sync_conflicts SET dismissed = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    // ==================================================================
    // Derived-merge for review scheduling (ADR-023)
    // ==================================================================

    /// Recompute a review card's FSRS scheduling state from the union of its
    /// append-only review logs and persist it (ADR-023 **derived-merge**).
    ///
    /// This is the pull-side reconciliation for review state: rather than
    /// value-merging a card's `stability`/`due_at`/`review_count` (which would
    /// double-count or drop concurrent reviews), the schedule is *recomputed* by
    /// folding the time-ordered log union through the deterministic FSRS
    /// scheduler. Every device holding the same logs converges to identical card
    /// state — so concurrent reviews keep due counts correct.
    ///
    /// No-op when the card row does not exist yet (a log can arrive before its
    /// card's metadata) or when the card has no logs (the ADR's "briefly stale
    /// until logs arrive" — any provisional value already written is left as-is).
    ///
    /// # Errors
    /// Returns a [`StorageError`] if a query fails.
    pub fn recompute_review_card(&self, card_id: &str) -> Result<(), StorageError> {
        let exists: bool = self
            .connection()
            .query_row(
                "SELECT 1 FROM review_cards WHERE id = ?1",
                params![card_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Ok(());
        }
        let logs = self.review_log_entries_for_card(card_id)?;
        if logs.is_empty() {
            return Ok(());
        }
        let derived = derive_card_state(&Scheduler::default_v5(), &logs);
        self.write_derived_card_state(card_id, &derived)
    }

    /// The `card_id` a review log belongs to, if the log exists locally.
    ///
    /// Used by the apply layer to find which card to recompute after appending
    /// a pulled review log.
    ///
    /// # Errors
    /// Returns a [`StorageError`] if the query fails.
    pub fn review_log_card_id(&self, log_id: &str) -> Result<Option<String>, StorageError> {
        let card = self
            .connection()
            .query_row(
                "SELECT card_id FROM review_logs WHERE id = ?1",
                params![log_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(card)
    }

    /// Read a card's review logs reduced to the `(id, reviewed_at, rating)` the
    /// derived-merge fold consumes. Rating is coerced to an integer regardless
    /// of column affinity; reviewed timestamps are parsed to epoch millis.
    fn review_log_entries_for_card(
        &self,
        card_id: &str,
    ) -> Result<Vec<ReviewLogEntry>, StorageError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT id, CAST(rating AS INTEGER), reviewed_at
             FROM review_logs WHERE card_id = ?1",
        )?;
        let rows = stmt.query_map(params![card_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (id, rating_val, reviewed_at) = row?;
            let Some(rating) = u32::try_from(rating_val).ok().and_then(Rating::from_value) else {
                continue;
            };
            entries.push(ReviewLogEntry {
                id,
                reviewed_at_ms: parse_epoch_millis(&reviewed_at),
                rating,
            });
        }
        Ok(entries)
    }

    /// Persist a derived schedule onto a review card, overriding the value-merged
    /// scalars. Preserves `content_item_id` and never touches the append-only
    /// logs. `due_at` keeps its existing value when the derivation yields none.
    fn write_derived_card_state(
        &self,
        card_id: &str,
        derived: &DerivedCard,
    ) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE review_cards SET
                state = ?2,
                stability = ?3,
                difficulty = ?4,
                due_at = COALESCE(?5, due_at),
                last_reviewed_at = ?6,
                review_count = ?7,
                lapse_count = ?8,
                scheduled_days = ?9
             WHERE id = ?1",
            params![
                card_id,
                derived.state.as_str(),
                derived.stability,
                derived.difficulty,
                derived.due_at_ms.map(millis_to_rfc3339),
                derived.last_reviewed_at_ms.map(millis_to_rfc3339),
                i64::from(derived.review_count),
                i64::from(derived.lapse_count),
                derived.scheduled_days,
            ],
        )?;
        Ok(())
    }

    // ==================================================================
    // Canonical field read / write (event body <-> typed tables)
    // ==================================================================

    /// Read an entity's synced fields as a `field -> value` map, or `None` if it
    /// does not exist locally. Field names are the wire/column names the merge
    /// policy and event bodies use.
    pub fn read_entity_fields(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<Option<FieldMap>, StorageError> {
        match entity_type {
            EntityType::Document => {
                self.read_columns("content_items", "id", entity_id, DOCUMENT_COLS)
            }
            EntityType::Tag => self.read_columns("tags", "id", entity_id, TAG_COLS),
            EntityType::Collection => {
                self.read_columns("collections", "id", entity_id, COLLECTION_COLS)
            }
            EntityType::Note => self.read_columns("notes", "id", entity_id, NOTE_COLS),
            EntityType::FeedSubscription => self.read_columns("feeds", "id", entity_id, FEED_COLS),
            EntityType::Settings => self.read_columns("settings", "key", entity_id, SETTINGS_COLS),
            EntityType::ReviewLog => {
                self.read_columns("review_logs", "id", entity_id, REVIEW_LOG_COLS)
            }
            EntityType::ReviewCard => {
                self.read_columns("review_cards", "id", entity_id, REVIEW_CARD_COLS)
            }
            EntityType::Highlight => self.read_highlight(entity_id),
            // Membership edges have no field row; their state is the set edge.
            EntityType::TagEdge | EntityType::CollectionEdge => Ok(None),
        }
    }

    /// Write an entity's fields into its typed table(s) for the given op.
    ///
    /// `Upsert` creates-or-replaces the provided fields; `FieldPatch` updates
    /// only the provided fields; `Delete` removes the row (cascading to
    /// dependent rows via foreign keys).
    pub fn write_entity_fields(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        fields: &FieldMap,
        op: Op,
    ) -> Result<(), StorageError> {
        if op == Op::Delete {
            return self.delete_entity_row(entity_type, entity_id);
        }
        // Field-map keys are interpolated into SQL as column identifiers by the
        // upsert helpers below, and they can originate from a remote (untrusted)
        // sync peer's change body. Reject any key that is not a known column for
        // this entity so a malicious peer cannot inject arbitrary SQL through a
        // crafted field name (fail closed).
        reject_unknown_columns(entity_type, fields)?;
        match entity_type {
            EntityType::Document => {
                self.upsert_single("content_items", "id", entity_id, fields, DOCUMENT_DEFAULTS)
            }
            EntityType::Tag => self.upsert_single("tags", "id", entity_id, fields, TAG_DEFAULTS),
            EntityType::Collection => {
                self.upsert_single("collections", "id", entity_id, fields, COLLECTION_DEFAULTS)
            }
            EntityType::Note => self.upsert_single("notes", "id", entity_id, fields, NOTE_DEFAULTS),
            EntityType::FeedSubscription => {
                self.upsert_single("feeds", "id", entity_id, fields, FEED_DEFAULTS)
            }
            EntityType::Settings => {
                self.upsert_single("settings", "key", entity_id, fields, SETTINGS_DEFAULTS)
            }
            EntityType::ReviewCard => self.upsert_single(
                "review_cards",
                "id",
                entity_id,
                fields,
                REVIEW_CARD_DEFAULTS,
            ),
            EntityType::ReviewLog => {
                self.insert_append_only("review_logs", "id", entity_id, fields, REVIEW_LOG_DEFAULTS)
            }
            EntityType::Highlight => self.upsert_highlight(entity_id, fields),
            EntityType::TagEdge | EntityType::CollectionEdge => Ok(()),
        }
    }

    /// Read a fixed set of columns of one row into a field map.
    fn read_columns(
        &self,
        table: &str,
        id_col: &str,
        id: &str,
        cols: &[&str],
    ) -> Result<Option<FieldMap>, StorageError> {
        let sql = format!(
            "SELECT {} FROM {table} WHERE {id_col} = ?1",
            cols.join(", ")
        );
        let conn = self.connection();
        let mut stmt = conn.prepare(&sql)?;
        let map = stmt
            .query_row(params![id], |row| {
                let mut map = FieldMap::new();
                for (i, col) in cols.iter().enumerate() {
                    let value = sql_to_json(row.get_ref(i)?);
                    map.insert((*col).to_owned(), value);
                }
                Ok(map)
            })
            .optional()?;
        Ok(map)
    }

    /// Read a highlight (`content_items` shell joined with `highlight_meta`).
    fn read_highlight(&self, id: &str) -> Result<Option<FieldMap>, StorageError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT ci.title, ci.status, hm.quote_text, hm.note, hm.color,
                    hm.position_start, hm.position_end, hm.source_item_id
             FROM content_items ci JOIN highlight_meta hm ON hm.content_item_id = ci.id
             WHERE ci.id = ?1",
        )?;
        let map = stmt
            .query_row(params![id], |row| {
                let mut map = FieldMap::new();
                for (i, col) in HIGHLIGHT_COLS.iter().enumerate() {
                    map.insert((*col).to_owned(), sql_to_json(row.get_ref(i)?));
                }
                Ok(map)
            })
            .optional()?;
        Ok(map)
    }

    /// Upsert a single-table entity from a field map, filling NOT NULL columns
    /// from `defaults` only when inserting a brand-new row.
    ///
    /// Uses update-then-insert rather than `INSERT … ON CONFLICT DO UPDATE`:
    /// `SQLite` validates NOT NULL constraints against the *candidate* insert row
    /// even when the statement resolves to `DO UPDATE`, so a single-field patch
    /// of a row whose other NOT NULL columns lack defaults (e.g.
    /// `notes.content_item_id`) would spuriously fail. Updating an existing row
    /// first sidesteps that entirely.
    fn upsert_single(
        &self,
        table: &str,
        id_col: &str,
        id: &str,
        fields: &FieldMap,
        defaults: &[(&str, &str)],
    ) -> Result<(), StorageError> {
        let conn = self.connection();
        // Update the provided fields on an existing row first.
        if !fields.is_empty() {
            let mut set_clause = Vec::with_capacity(fields.len());
            let mut update_vals: Vec<rusqlite::types::Value> =
                vec![rusqlite::types::Value::Text(id.to_owned())];
            for (col, value) in fields {
                set_clause.push(format!("{col} = ?{}", update_vals.len() + 1));
                update_vals.push(json_to_sql(value));
            }
            let sql = format!(
                "UPDATE {table} SET {} WHERE {id_col} = ?1",
                set_clause.join(", ")
            );
            let affected = conn.execute(&sql, rusqlite::params_from_iter(update_vals))?;
            if affected > 0 {
                return Ok(());
            }
        }
        // Row absent: insert id + provided fields + any default column missing.
        let mut insert_cols: Vec<String> = vec![id_col.to_owned()];
        let mut insert_vals: Vec<rusqlite::types::Value> =
            vec![rusqlite::types::Value::Text(id.to_owned())];
        for (col, value) in fields {
            insert_cols.push(col.clone());
            insert_vals.push(json_to_sql(value));
        }
        for (col, default) in defaults {
            if !fields.contains_key(*col) {
                insert_cols.push((*col).to_owned());
                insert_vals.push(rusqlite::types::Value::Text((*default).to_owned()));
            }
        }
        let placeholders: Vec<String> = (1..=insert_cols.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT({id_col}) DO NOTHING",
            insert_cols.join(", "),
            placeholders.join(", "),
        );
        conn.execute(&sql, rusqlite::params_from_iter(insert_vals))?;
        Ok(())
    }

    /// Insert an append-only row (review log): idempotent by id, never updated.
    fn insert_append_only(
        &self,
        table: &str,
        id_col: &str,
        id: &str,
        fields: &FieldMap,
        defaults: &[(&str, &str)],
    ) -> Result<(), StorageError> {
        let mut cols: Vec<String> = vec![id_col.to_owned()];
        let mut vals: Vec<rusqlite::types::Value> =
            vec![rusqlite::types::Value::Text(id.to_owned())];
        for (col, value) in fields {
            cols.push(col.clone());
            vals.push(json_to_sql(value));
        }
        for (col, default) in defaults {
            if !fields.contains_key(*col) {
                cols.push((*col).to_owned());
                vals.push(rusqlite::types::Value::Text((*default).to_owned()));
            }
        }
        let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT OR IGNORE INTO {table} ({}) VALUES ({})",
            cols.join(", "),
            placeholders.join(", "),
        );
        self.connection()
            .execute(&sql, rusqlite::params_from_iter(vals))?;
        Ok(())
    }

    /// Upsert a highlight: the `content_items` shell plus its `highlight_meta`.
    fn upsert_highlight(&self, id: &str, fields: &FieldMap) -> Result<(), StorageError> {
        let title = fields.get("title").and_then(Value::as_str).unwrap_or("");
        let status = fields
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("inbox");
        self.connection().execute(
            "INSERT INTO content_items (id, title, content_type, status)
             VALUES (?1, ?2, 'highlight', ?3)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, status = excluded.status",
            params![id, title, status],
        )?;
        // highlight_meta carries the annotation itself.
        let mut meta = fields.clone();
        meta.remove("title");
        meta.remove("status");
        // quote_text is NOT NULL; default to empty when patching without it.
        let mut insert_cols = vec!["content_item_id".to_owned()];
        let mut insert_vals: Vec<rusqlite::types::Value> =
            vec![rusqlite::types::Value::Text(id.to_owned())];
        for (col, value) in &meta {
            insert_cols.push(col.clone());
            insert_vals.push(json_to_sql(value));
        }
        if !meta.contains_key("quote_text") {
            insert_cols.push("quote_text".to_owned());
            insert_vals.push(rusqlite::types::Value::Text(String::new()));
        }
        let update_clause: Vec<String> =
            meta.keys().map(|c| format!("{c} = excluded.{c}")).collect();
        let placeholders: Vec<String> = (1..=insert_cols.len()).map(|i| format!("?{i}")).collect();
        let sql = if update_clause.is_empty() {
            format!(
                "INSERT INTO highlight_meta ({}) VALUES ({}) \
                 ON CONFLICT(content_item_id) DO NOTHING",
                insert_cols.join(", "),
                placeholders.join(", "),
            )
        } else {
            format!(
                "INSERT INTO highlight_meta ({}) VALUES ({}) \
                 ON CONFLICT(content_item_id) DO UPDATE SET {}",
                insert_cols.join(", "),
                placeholders.join(", "),
                update_clause.join(", "),
            )
        };
        self.connection()
            .execute(&sql, rusqlite::params_from_iter(insert_vals))?;
        Ok(())
    }

    /// Delete an entity's canonical row (foreign keys cascade to children).
    fn delete_entity_row(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<(), StorageError> {
        let (table, id_col) = match entity_type {
            EntityType::Document | EntityType::Highlight => ("content_items", "id"),
            EntityType::Tag => ("tags", "id"),
            EntityType::Collection => ("collections", "id"),
            EntityType::Note => ("notes", "id"),
            EntityType::FeedSubscription => ("feeds", "id"),
            EntityType::Settings => ("settings", "key"),
            EntityType::ReviewCard => ("review_cards", "id"),
            // Review logs are append-only; edges are handled via set state.
            EntityType::ReviewLog | EntityType::TagEdge | EntityType::CollectionEdge => {
                return Ok(());
            }
        };
        self.connection().execute(
            &format!("DELETE FROM {table} WHERE {id_col} = ?1"),
            params![entity_id],
        )?;
        Ok(())
    }

    /// Apply a membership edge's present/absent state to the canonical join table.
    pub fn apply_edge_membership(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        present: bool,
    ) -> Result<(), StorageError> {
        let (left, right) = split_edge_id(entity_id)?;
        match entity_type {
            EntityType::TagEdge => {
                if present {
                    self.connection().execute(
                        "INSERT OR IGNORE INTO content_item_tags (content_item_id, tag_id)
                         VALUES (?1, ?2)",
                        params![left, right],
                    )?;
                } else {
                    self.connection().execute(
                        "DELETE FROM content_item_tags WHERE content_item_id = ?1 AND tag_id = ?2",
                        params![left, right],
                    )?;
                }
            }
            EntityType::CollectionEdge => {
                if present {
                    self.connection().execute(
                        "INSERT OR IGNORE INTO content_item_collections
                            (content_item_id, collection_id) VALUES (?1, ?2)",
                        params![left, right],
                    )?;
                } else {
                    self.connection().execute(
                        "DELETE FROM content_item_collections
                         WHERE content_item_id = ?1 AND collection_id = ?2",
                        params![left, right],
                    )?;
                }
            }
            _ => {
                return Err(StorageError::Generic(
                    "apply_edge_membership called with a non-edge entity".to_owned(),
                ));
            }
        }
        Ok(())
    }

    // ==================================================================
    // Local mutation emission (tracked write)
    // ==================================================================

    /// Emit a local mutation: write the canonical row(s), stamp per-field
    /// clocks, and enqueue an outbox row — all atomically. Returns the new
    /// change's `change_id`, or `None` when sync is not enabled (device id
    /// unset), in which case only the canonical write happens.
    ///
    /// `entity_id` is the entity's id (or the `left:right` edge id). For edges,
    /// pass a single `present` field (`true`/`false`); the canonical join table
    /// and observed-remove state are updated accordingly.
    pub fn emit_change(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        op: Op,
        fields: FieldMap,
        blob_manifest: Vec<BlobManifestEntry>,
        now_millis: u64,
    ) -> Result<Option<String>, StorageError> {
        self.in_transaction(|db| {
            let device_id = db.sync_state()?.device_id;
            let Some(device_id) = device_id else {
                // Sync disabled: apply the canonical write only.
                db.apply_local_canonical(entity_type, entity_id, op, &fields)?;
                return Ok(None);
            };
            let _ = device_id;
            let clock = db.tick_local_hlc(now_millis)?;

            // base_version = the version the writer observed, so a remote applier
            // can tell a causal update from genuine concurrency. For a conflict-
            // copy edit it is the clock we hold for that field; for a delete it is
            // the authored-prose version the deleter saw, so a concurrent prose
            // edit the delete never observed is preserved rather than erased
            // (ADR-023 annotation policy). `None` for pure LWW / creates.
            let base_version = if op == Op::Delete {
                db.observed_prose_version(entity_type, entity_id)?
            } else {
                db.base_version_for(entity_type, entity_id, &fields)?
            };

            db.apply_local_canonical(entity_type, entity_id, op, &fields)?;

            // Stamp clocks and set edge/tombstone state.
            if op == Op::Delete {
                db.set_tombstone(entity_type, entity_id, &clock)?;
            } else {
                for field in fields.keys() {
                    db.set_entity_clock(entity_type, entity_id, field, &clock)?;
                }
                if matches!(
                    entity_type,
                    EntityType::TagEdge | EntityType::CollectionEdge
                ) {
                    let is_add = fields
                        .get("present")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let member = db.set_edge(entity_type, entity_id)?;
                    let outcome =
                        pergamon_core::sync::merge::merge_set_member(&member, is_add, &clock);
                    db.save_set_edge(entity_type, entity_id, &outcome.member)?;
                }
            }

            let body = ChangeBody {
                entity_type,
                entity_id: entity_id.to_owned(),
                op,
                clock,
                base_version,
                fields,
                blob_manifest,
            };
            let change_id = db.enqueue_outbox(&body)?;
            Ok(Some(change_id))
        })
    }

    /// Write a local mutation's canonical rows (no clocks / outbox). Shared by
    /// emit whether or not sync is enabled.
    fn apply_local_canonical(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        op: Op,
        fields: &FieldMap,
    ) -> Result<(), StorageError> {
        if matches!(
            entity_type,
            EntityType::TagEdge | EntityType::CollectionEdge
        ) {
            let present = fields
                .get("present")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            return self.apply_edge_membership(entity_type, entity_id, present);
        }
        self.write_entity_fields(entity_type, entity_id, fields, op)
    }

    /// Compute the observed base version for a change: the current clock of the
    /// first conflict-copy field being written, if any.
    fn base_version_for(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        fields: &FieldMap,
    ) -> Result<Option<Hlc>, StorageError> {
        for field in fields.keys() {
            if strategy_for(entity_type, field) == ConflictStrategy::ConflictCopy {
                return self.entity_clock(entity_type, entity_id, field);
            }
        }
        Ok(None)
    }

    /// The authored-prose version a delete observed: the greatest clock among
    /// the entity's conflict-copy (prose) fields. A remote applier compares a
    /// concurrent prose edit against this to decide whether the delete saw it —
    /// if not, the edit is preserved in the conflict inbox instead of erased.
    fn observed_prose_version(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<Option<Hlc>, StorageError> {
        let Some(fields) = self.read_entity_fields(entity_type, entity_id)? else {
            return Ok(None);
        };
        let mut max: Option<Hlc> = None;
        for field in fields.keys() {
            if strategy_for(entity_type, field) != ConflictStrategy::ConflictCopy {
                continue;
            }
            if let Some(clock) = self.entity_clock(entity_type, entity_id, field)? {
                max = match max {
                    Some(current) if current >= clock => Some(current),
                    _ => Some(clock),
                };
            }
        }
        Ok(max)
    }
}

/// Column sets read for each single-table entity (wire/column names).
const DOCUMENT_COLS: &[&str] = &[
    "url",
    "title",
    "author",
    "content_type",
    "status",
    "content_text",
    "excerpt",
    "published_at",
    "read_at",
];
const TAG_COLS: &[&str] = &["name"];
const COLLECTION_COLS: &[&str] = &["name", "parent_id", "sort_order"];
const NOTE_COLS: &[&str] = &["content_item_id", "body"];
const FEED_COLS: &[&str] = &["title", "url", "site_url", "description", "folder_id"];
const SETTINGS_COLS: &[&str] = &["value"];
const HIGHLIGHT_COLS: &[&str] = &[
    "title",
    "status",
    "quote_text",
    "note",
    "color",
    "position_start",
    "position_end",
    "source_item_id",
];
const REVIEW_CARD_COLS: &[&str] = &[
    "content_item_id",
    "state",
    "stability",
    "difficulty",
    "due_at",
    "last_reviewed_at",
    "review_count",
    "lapse_count",
    "scheduled_days",
];
const REVIEW_LOG_COLS: &[&str] = &[
    "card_id",
    "rating",
    "state_before",
    "stability_before",
    "difficulty_before",
    "state_after",
    "stability_after",
    "difficulty_after",
    "elapsed_days",
    "scheduled_days",
    "reviewed_at",
];

/// NOT NULL column defaults used only when inserting a brand-new row.
const DOCUMENT_DEFAULTS: &[(&str, &str)] = &[
    ("title", ""),
    ("content_type", "article"),
    ("status", "inbox"),
];
const TAG_DEFAULTS: &[(&str, &str)] = &[("name", "")];
const COLLECTION_DEFAULTS: &[(&str, &str)] = &[("name", "")];
const NOTE_DEFAULTS: &[(&str, &str)] = &[("body", "")];
const FEED_DEFAULTS: &[(&str, &str)] = &[("title", ""), ("url", "")];
const SETTINGS_DEFAULTS: &[(&str, &str)] = &[("value", "")];
const REVIEW_CARD_DEFAULTS: &[(&str, &str)] = &[("state", "new")];
const REVIEW_LOG_DEFAULTS: &[(&str, &str)] = &[];

/// The set of field/column names a sync change may legally write for an entity.
///
/// This is the same wire/column vocabulary the read path uses, and it is the
/// allowlist that guards the SQL-identifier interpolation in the upsert helpers.
/// Edges carry no single-table columns.
const fn allowed_columns(entity_type: EntityType) -> &'static [&'static str] {
    match entity_type {
        EntityType::Document => DOCUMENT_COLS,
        EntityType::Tag => TAG_COLS,
        EntityType::Collection => COLLECTION_COLS,
        EntityType::Note => NOTE_COLS,
        EntityType::FeedSubscription => FEED_COLS,
        EntityType::Settings => SETTINGS_COLS,
        EntityType::ReviewCard => REVIEW_CARD_COLS,
        EntityType::ReviewLog => REVIEW_LOG_COLS,
        EntityType::Highlight => HIGHLIGHT_COLS,
        EntityType::TagEdge | EntityType::CollectionEdge => &[],
    }
}

/// Reject a field map that names any column outside the entity's allowlist.
///
/// The upsert helpers interpolate field-map keys directly into SQL as column
/// identifiers (bound parameters only cover the *values*), and those keys can
/// come from a remote, untrusted sync peer. Validating them against the fixed
/// per-entity column set closes that identifier-injection vector: a crafted key
/// such as `"content_text = (SELECT …), title"` is rejected before it can reach
/// a `format!`-built statement.
fn reject_unknown_columns(entity_type: EntityType, fields: &FieldMap) -> Result<(), StorageError> {
    let allowed = allowed_columns(entity_type);
    for key in fields.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(StorageError::Constraint(format!(
                "unknown sync column `{key}` for {entity_type:?}"
            )));
        }
    }
    Ok(())
}

/// Split a composite edge id (`left:right`) into its two entity ids.
fn split_edge_id(edge_id: &str) -> Result<(String, String), StorageError> {
    edge_id.split_once(':').map_or_else(
        || {
            Err(StorageError::Generic(format!(
                "malformed edge id {edge_id}"
            )))
        },
        |(l, r)| Ok((l.to_owned(), r.to_owned())),
    )
}

/// Parse an RFC 3339 review timestamp to epoch milliseconds. Falls back to `0`
/// for an unparseable value, so a malformed row still folds deterministically
/// rather than panicking on the sync path.
fn parse_epoch_millis(s: &str) -> i64 {
    OffsetDateTime::parse(s, &Rfc3339).map_or(0, |t| {
        i64::try_from(t.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
    })
}

/// Format epoch milliseconds back to an RFC 3339 timestamp for a card's
/// `due_at` / `last_reviewed_at` columns.
fn millis_to_rfc3339(ms: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// Convert a `SQLite` value reference into a JSON value for a field map.
fn sql_to_json(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => Value::from(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
    }
}

/// Rebuild an optional clock from three nullable columns.
fn optional_clock(wall: Option<i64>, counter: Option<i64>, device: Option<String>) -> Option<Hlc> {
    match (wall, counter, device) {
        (Some(w), Some(c), Some(d)) => Some(Hlc::new(
            u64::try_from(w).unwrap_or(0),
            u32::try_from(c).unwrap_or(0),
            d,
        )),
        _ => None,
    }
}

/// Split an optional clock into its three nullable column values.
fn split_clock(clock: Option<&Hlc>) -> (Option<i64>, Option<i64>, Option<String>) {
    clock.map_or((None, None, None), |c| {
        (
            Some(i64::try_from(c.wall_millis).unwrap_or(i64::MAX)),
            Some(i64::from(c.counter)),
            Some(c.device_id.clone()),
        )
    })
}

/// Convert a JSON value to a SQLite-storable value (text/int/real/null).
pub(crate) fn json_to_sql(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Sql::Real(n.as_f64().unwrap_or(0.0)), Sql::Integer),
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}
