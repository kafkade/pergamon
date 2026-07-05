// SPDX-License-Identifier: Apache-2.0

//! Applying a decrypted [`ChangeBody`] onto local storage through the ADR-023
//! merge policy.
//!
//! This is the pull-side counterpart to [`Database::emit_change`]. For each
//! pulled change it resolves every field (or membership edge, or delete) against
//! the local state and the per-field hybrid-logical-clocks, writing the merged
//! result and recording conflict copies and tombstones. It is deterministic and
//! commutative: applying the same set of changes in any order on any device
//! yields byte-identical state — the convergence guarantee issue #126 requires.
//!
//! [`Database::emit_change`]: pergamon_storage::Database::emit_change

use serde_json::{Map, Value};

use pergamon_core::sync::event::{ChangeBody, EntityType, Op};
use pergamon_core::sync::merge::{
    FieldMerge, MergeDecision, merge_field, merge_set_member, strategy_for,
};
use pergamon_storage::Database;

use crate::error::Result;

/// Apply one decrypted change to local storage, merging per ADR-023.
///
/// The caller is responsible for idempotency (skipping already-applied
/// `change_id`s), for observing the change's clock into the local HLC, and for
/// running this inside a transaction.
///
/// # Errors
/// Returns a [`SyncError`](crate::SyncError) if any storage operation fails.
pub fn apply_change(db: &Database, body: &ChangeBody) -> Result<()> {
    match body.entity_type {
        EntityType::TagEdge | EntityType::CollectionEdge => apply_edge(db, body),
        _ if body.op == Op::Delete => apply_delete(db, body),
        EntityType::ReviewLog => apply_append_only(db, body),
        _ => apply_fields(db, body),
    }
}

/// Apply a membership edge add/remove via observed-remove set merge.
fn apply_edge(db: &Database, body: &ChangeBody) -> Result<()> {
    let is_add = body
        .fields
        .get("present")
        .and_then(Value::as_bool)
        .unwrap_or(body.op != Op::Delete);
    let local = db.set_edge(body.entity_type, &body.entity_id)?;
    let outcome = merge_set_member(&local, is_add, &body.clock);
    db.save_set_edge(body.entity_type, &body.entity_id, &outcome.member)?;
    db.apply_edge_membership(body.entity_type, &body.entity_id, outcome.present)?;
    Ok(())
}

/// Apply a delete: record the tombstone and remove the row when the delete
/// dominates every field write we hold (delete-wins by HLC).
fn apply_delete(db: &Database, body: &ChangeBody) -> Result<()> {
    db.set_tombstone(body.entity_type, &body.entity_id, &body.clock)?;
    if max_field_clock_dominated(db, body)? {
        db.write_entity_fields(body.entity_type, &body.entity_id, &Map::new(), Op::Delete)?;
    }
    Ok(())
}

/// Whether every per-field clock of the entity is `<= body.clock` (so a delete
/// at `body.clock` is not overridden by a newer concurrent edit).
fn max_field_clock_dominated(db: &Database, body: &ChangeBody) -> Result<bool> {
    let Some(fields) = db.read_entity_fields(body.entity_type, &body.entity_id)? else {
        return Ok(true);
    };
    for field in fields.keys() {
        if let Some(clock) = db.entity_clock(body.entity_type, &body.entity_id, field)?
            && clock > body.clock
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Apply an append-only entity (review log): idempotent insert by id.
fn apply_append_only(db: &Database, body: &ChangeBody) -> Result<()> {
    db.write_entity_fields(body.entity_type, &body.entity_id, &body.fields, Op::Upsert)?;
    Ok(())
}

/// Apply a normal per-field change (upsert or field patch).
fn apply_fields(db: &Database, body: &ChangeBody) -> Result<()> {
    // A delete that strictly dominates this write means the entity is gone.
    if let Some(dead) = db.tombstone(body.entity_type, &body.entity_id)?
        && dead > body.clock
    {
        return Ok(());
    }

    let local_fields = db.read_entity_fields(body.entity_type, &body.entity_id)?;

    for (field, incoming_value) in &body.fields {
        let strategy = strategy_for(body.entity_type, field);
        let local_value = local_fields.as_ref().and_then(|m| m.get(field));
        let local_clock = db.entity_clock(body.entity_type, &body.entity_id, field)?;
        let local = match (local_value, local_clock.as_ref()) {
            (Some(v), Some(c)) => Some((v, c)),
            _ => None,
        };
        let merge = FieldMerge {
            local,
            incoming_value,
            incoming_clock: &body.clock,
            base_version: body.base_version.as_ref(),
        };
        match merge_field(strategy, &merge) {
            MergeDecision::KeepLocal => {}
            MergeDecision::TakeIncoming => {
                write_one_field(db, body, field, incoming_value)?;
                db.set_entity_clock(body.entity_type, &body.entity_id, field, &body.clock)?;
            }
            MergeDecision::ConflictCopy {
                winner,
                winner_clock,
                loser,
                loser_clock,
            } => {
                write_one_field(db, body, field, &winner)?;
                db.set_entity_clock(body.entity_type, &body.entity_id, field, &winner_clock)?;
                db.insert_conflict(
                    body.entity_type,
                    &body.entity_id,
                    field,
                    &loser,
                    &loser_clock,
                )?;
            }
        }
    }
    Ok(())
}

/// Write a single resolved field into its typed table.
fn write_one_field(db: &Database, body: &ChangeBody, field: &str, value: &Value) -> Result<()> {
    let mut one = Map::new();
    one.insert(field.to_owned(), value.clone());
    db.write_entity_fields(body.entity_type, &body.entity_id, &one, Op::FieldPatch)?;
    Ok(())
}
