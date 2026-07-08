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
//! The per-entity strategies (ADR-023) handled here are:
//!
//! - **Documents / config / entities** — per-field LWW or conflict-copy via
//!   [`merge_field`], with `field_patch` granularity.
//! - **Tag / collection membership** — observed-remove set merge.
//! - **Deletes** — tombstone, delete-wins-by-HLC, but authored prose the delete
//!   never observed is preserved in the conflict inbox, never silently erased.
//! - **Review logs** — append-only, auto-merged by id.
//! - **Review cards** — *derived-merge*: the FSRS schedule is recomputed from the
//!   review-log union, never value-merged, so concurrent reviews keep due counts
//!   correct.
//!
//! [`Database::emit_change`]: pergamon_storage::Database::emit_change

use serde_json::{Map, Value};

use pergamon_core::sync::event::{ChangeBody, EntityType, Op};
use pergamon_core::sync::merge::{
    ConflictStrategy, FieldMerge, MergeDecision, merge_field, merge_set_member, strategy_for,
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
        EntityType::ReviewLog => apply_review_log(db, body),
        EntityType::ReviewCard => apply_review_card(db, body),
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
///
/// Before removing, any authored-prose field the delete did **not** observe (a
/// concurrent edit newer than the deleter's observed prose version) is preserved
/// in the conflict inbox, so a delete never silently erases an edit the deleter
/// never saw (ADR-023).
fn apply_delete(db: &Database, body: &ChangeBody) -> Result<()> {
    db.set_tombstone(body.entity_type, &body.entity_id, &body.clock)?;
    preserve_unobserved_prose(db, body)?;
    if max_field_clock_dominated(db, body)? {
        db.write_entity_fields(body.entity_type, &body.entity_id, &Map::new(), Op::Delete)?;
    }
    Ok(())
}

/// Record, in the conflict inbox, every authored-prose field whose current value
/// the incoming delete did not observe (`field_clock > delete.base_version`).
fn preserve_unobserved_prose(db: &Database, body: &ChangeBody) -> Result<()> {
    let Some(fields) = db.read_entity_fields(body.entity_type, &body.entity_id)? else {
        return Ok(());
    };
    let observed = body.base_version.as_ref();
    for (field, value) in &fields {
        if strategy_for(body.entity_type, field) != ConflictStrategy::ConflictCopy {
            continue;
        }
        let Some(clock) = db.entity_clock(body.entity_type, &body.entity_id, field)? else {
            continue;
        };
        // The delete observed this prose iff its base_version is at least the
        // field's clock. An unobserved (concurrent) edit is preserved.
        let observed_this = observed.is_some_and(|base| &clock <= base);
        if !observed_this {
            db.insert_conflict(body.entity_type, &body.entity_id, field, value, &clock)?;
        }
    }
    Ok(())
}

/// Whether the delete at `body.clock` dominates every *non-prose* field write we
/// hold, so the row may be removed (delete-wins-live).
///
/// Authored-prose (`ConflictCopy`) fields are excluded: a concurrent prose edit
/// never keeps the annotation alive — it is preserved in the conflict inbox by
/// [`preserve_unobserved_prose`] instead. A newer non-prose LWW edit, however,
/// still beats the delete and keeps the entity (existing edit-wins semantics).
fn max_field_clock_dominated(db: &Database, body: &ChangeBody) -> Result<bool> {
    let Some(fields) = db.read_entity_fields(body.entity_type, &body.entity_id)? else {
        return Ok(true);
    };
    for field in fields.keys() {
        if strategy_for(body.entity_type, field) == ConflictStrategy::ConflictCopy {
            continue;
        }
        if let Some(clock) = db.entity_clock(body.entity_type, &body.entity_id, field)?
            && clock > body.clock
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Apply an append-only review log (idempotent insert by id), then recompute the
/// owning card's schedule from the log union (ADR-023 derived-merge).
fn apply_review_log(db: &Database, body: &ChangeBody) -> Result<()> {
    db.write_entity_fields(body.entity_type, &body.entity_id, &body.fields, Op::Upsert)?;
    let card_id = body
        .fields
        .get("card_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let card_id = match card_id {
        Some(id) => Some(id),
        None => db.review_log_card_id(&body.entity_id)?,
    };
    if let Some(card_id) = card_id {
        db.recompute_review_card(&card_id)?;
    }
    Ok(())
}

/// Apply a review-card upsert: seed the card row with its identity/provisional
/// fields, then override the scheduling scalars with the value **derived** from
/// the review-log union whenever logs exist. The card's stored `stability`,
/// `due_at`, `review_count`, … are never value-merged (ADR-023), so concurrent
/// reviews on two devices never double-count or drop a review.
fn apply_review_card(db: &Database, body: &ChangeBody) -> Result<()> {
    db.write_entity_fields(body.entity_type, &body.entity_id, &body.fields, Op::Upsert)?;
    db.recompute_review_card(&body.entity_id)?;
    Ok(())
}

/// Apply a normal per-field change (upsert or field patch).
fn apply_fields(db: &Database, body: &ChangeBody) -> Result<()> {
    let tombstone = db.tombstone(body.entity_type, &body.entity_id)?;
    let local_fields = db.read_entity_fields(body.entity_type, &body.entity_id)?;

    // Materialize a brand-new row in one shot so multi-column NOT NULL / foreign
    // key constraints (e.g. `notes.content_item_id`) are satisfied before the
    // per-field merge loop patches fields individually. Skipped when the entity
    // is tombstoned — a delete is never resurrected wholesale here.
    if local_fields.is_none() && tombstone.is_none() {
        db.write_entity_fields(body.entity_type, &body.entity_id, &body.fields, Op::Upsert)?;
    }

    for (field, incoming_value) in &body.fields {
        let strategy = strategy_for(body.entity_type, field);

        if let Some(dead) = tombstone.as_ref() {
            // The entity is tombstoned. Authored prose is never silently lost:
            // preserve the edit in the conflict inbox and keep the entity
            // deleted (no partial-row resurrection). Low-stakes fields dominated
            // by the delete are dropped; a non-prose field newer than the delete
            // falls through to a normal merge (may re-materialize the row).
            if strategy == ConflictStrategy::ConflictCopy {
                db.insert_conflict(
                    body.entity_type,
                    &body.entity_id,
                    field,
                    incoming_value,
                    &body.clock,
                )?;
                continue;
            }
            if dead > &body.clock {
                continue;
            }
        }

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
