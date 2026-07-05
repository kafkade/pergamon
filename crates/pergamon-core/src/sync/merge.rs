//! The ADR-023 conflict policy: table-driven, per-field merge strategies over
//! the ADR-022 hybrid logical clock.
//!
//! Everything here is pure and deterministic: given the same local state and
//! the same incoming change, every device reaches the same result regardless of
//! pull order. The four strategies are:
//!
//! - **LWW** — keep the value with the greater HLC; discard the other. Only for
//!   low-stakes scalars (triage flags, names, config).
//! - **Set-union + observed-remove tombstone** — membership edges; concurrent
//!   adds both survive, a delete only dominates adds it causally precedes, and a
//!   later re-add resurrects.
//! - **Derived-merge** — review scheduling state is *recomputed* from the
//!   append-only log union, never value-merged. Handled by the sync-apply layer;
//!   [`merge_field`] falls back to LWW for such fields as a safe default.
//! - **Conflict-copy** — authored prose (document body, note/annotation body):
//!   the HLC winner stays live, the loser is preserved as a sibling and surfaced
//!   in the conflict inbox. Never silently dropped.
//!
//! See ADR-023 for the full per-entity table this module encodes.

use serde_json::Value;

use super::event::EntityType;
use super::hlc::Hlc;

/// Which of the four ADR-023 strategies governs a given field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStrategy {
    /// Last-writer-wins by HLC. Lossy by design; low-stakes scalars only.
    Lww,
    /// Set-union with observed-remove tombstones (membership edges).
    SetUnionTombstone,
    /// Recomputed from an append-only history (review scheduling state).
    DerivedMerge,
    /// Keep both bodies: HLC winner is live, loser becomes a conflict copy.
    ConflictCopy,
}

/// Resolve the strategy for a `(entity_type, field)` pair per the ADR-023 table.
///
/// The only fields that are ever surfaced to the user (conflict-copy) are
/// authored prose bodies; everything else auto-merges.
#[must_use]
#[allow(clippy::match_same_arms)] // arms kept separate to mirror the ADR-023 table
pub fn strategy_for(entity: EntityType, field: &str) -> ConflictStrategy {
    match entity {
        // A document mixes low-stakes scalars (LWW) with one authored-prose
        // body field (conflict-copy). Extraction-derived and metadata scalars
        // are all LWW — the newest-clock extraction wins, never a conflict.
        EntityType::Document => match field {
            // `content_text` is the authored/extracted prose body (its storage
            // column); `body`/`notes` are accepted as aliases for forward-compat.
            "content_text" | "body" | "notes" => ConflictStrategy::ConflictCopy,
            _ => ConflictStrategy::Lww,
        },
        // Tag / collection *entities* are per-field LWW (a name is not prose).
        EntityType::Tag | EntityType::Collection => ConflictStrategy::Lww,
        // Membership edges are the set CRDT.
        EntityType::TagEdge | EntityType::CollectionEdge => ConflictStrategy::SetUnionTombstone,
        // A highlight's attached note is authored prose (conflict-copy); its
        // anchor is immutable provenance and its color is a low-stakes scalar.
        EntityType::Highlight => match field {
            "note" => ConflictStrategy::ConflictCopy,
            _ => ConflictStrategy::Lww,
        },
        // A note's body is authored prose.
        EntityType::Note => match field {
            "body" => ConflictStrategy::ConflictCopy,
            _ => ConflictStrategy::Lww,
        },
        // A review card's lifecycle flag is LWW; its scheduling scalars are
        // derived from the log union and never value-merged.
        EntityType::ReviewCard => match field {
            "enabled" => ConflictStrategy::Lww,
            _ => ConflictStrategy::DerivedMerge,
        },
        // Review logs are append-only and idempotent by id; a field strategy is
        // never actually consulted, but LWW is the harmless default.
        EntityType::ReviewLog => ConflictStrategy::Lww,
        // Mutable config, no authored prose.
        EntityType::FeedSubscription | EntityType::Settings => ConflictStrategy::Lww,
    }
}

/// Inputs to a single-field merge.
#[derive(Debug, Clone)]
pub struct FieldMerge<'a> {
    /// The local value and the HLC that last wrote it, if the field exists.
    pub local: Option<(&'a Value, &'a Hlc)>,
    /// The incoming value.
    pub incoming_value: &'a Value,
    /// The HLC of the incoming change.
    pub incoming_clock: &'a Hlc,
    /// The field version the incoming writer observed. Used only to distinguish
    /// a causal fast-forward from genuine concurrency for conflict-copy fields.
    pub base_version: Option<&'a Hlc>,
}

/// The outcome of merging one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeDecision {
    /// The local value wins; do nothing.
    KeepLocal,
    /// The incoming value wins; write it and stamp the field with its clock.
    TakeIncoming,
    /// A concurrent edit to authored prose: keep the winner live and preserve
    /// the loser as a conflict copy.
    ConflictCopy {
        /// The value that becomes/stays the live one (greater HLC).
        winner: Value,
        /// The clock stamped on the live value.
        winner_clock: Hlc,
        /// The value preserved as a sibling conflict copy.
        loser: Value,
        /// The clock of the losing value.
        loser_clock: Hlc,
    },
}

/// Merge a single field under the given strategy.
///
/// `DerivedMerge` is not resolved here (review scheduling is recomputed by the
/// apply layer from the log union); it falls back to LWW so a stray call is
/// still convergent.
#[must_use]
pub fn merge_field(strategy: ConflictStrategy, m: &FieldMerge<'_>) -> MergeDecision {
    match strategy {
        ConflictStrategy::ConflictCopy => merge_conflict_copy(m),
        // LWW, DerivedMerge fallback, and (defensively) SetUnionTombstone on a
        // scalar all reduce to "greater HLC wins".
        _ => merge_lww(m),
    }
}

/// Last-writer-wins: take the incoming value iff its clock is greater than the
/// local field clock (or the field does not exist locally).
fn merge_lww(m: &FieldMerge<'_>) -> MergeDecision {
    match m.local {
        None => MergeDecision::TakeIncoming,
        Some((_, local_clock)) => {
            if m.incoming_clock > local_clock {
                MergeDecision::TakeIncoming
            } else {
                MergeDecision::KeepLocal
            }
        }
    }
}

/// Conflict-copy: never silently drop authored prose.
///
/// A *causal fast-forward* (the writer observed exactly the version we hold)
/// applies cleanly. Genuine concurrency keeps the HLC winner live and preserves
/// the loser as a conflict copy — unless the two values are identical, in which
/// case there is nothing to reconcile.
fn merge_conflict_copy(m: &FieldMerge<'_>) -> MergeDecision {
    let Some((local_value, local_clock)) = m.local else {
        // First writer for this field: no conflict possible.
        return MergeDecision::TakeIncoming;
    };

    // Idempotent re-delivery or an identical concurrent edit: nothing to do.
    if local_value == m.incoming_value {
        return if m.incoming_clock > local_clock {
            MergeDecision::TakeIncoming
        } else {
            MergeDecision::KeepLocal
        };
    }

    // Causal fast-forward: the writer edited the exact version we hold.
    let is_causal = m.base_version.is_some_and(|base| base == local_clock);
    if is_causal {
        return MergeDecision::TakeIncoming;
    }

    // Genuine concurrency: keep both, HLC winner live.
    if m.incoming_clock > local_clock {
        MergeDecision::ConflictCopy {
            winner: m.incoming_value.clone(),
            winner_clock: m.incoming_clock.clone(),
            loser: local_value.clone(),
            loser_clock: local_clock.clone(),
        }
    } else {
        MergeDecision::ConflictCopy {
            winner: local_value.clone(),
            winner_clock: local_clock.clone(),
            loser: m.incoming_value.clone(),
            loser_clock: m.incoming_clock.clone(),
        }
    }
}

/// The observed-remove state of one set member (a membership edge): the latest
/// clock at which it was added and the latest at which it was removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetMember {
    /// Greatest HLC of an add for this member, if ever added.
    pub add_clock: Option<Hlc>,
    /// Greatest HLC of a remove for this member, if ever removed.
    pub remove_clock: Option<Hlc>,
}

impl SetMember {
    /// Whether the member is currently present. **Add-wins**: the member is in
    /// the set iff it has an add whose clock is not dominated by a later remove.
    #[must_use]
    pub fn is_present(&self) -> bool {
        match (&self.add_clock, &self.remove_clock) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(add), Some(remove)) => add > remove,
        }
    }
}

/// The result of folding one add/remove into a set member's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetMergeOutcome {
    /// The updated member state.
    pub member: SetMember,
    /// Whether the member is present after the merge.
    pub present: bool,
}

/// Fold an incoming membership add or remove into a member's observed-remove
/// state (ADR-023 set-union with observed-remove tombstones).
///
/// Adds and removes each keep only their greatest clock, so the operation is
/// commutative, associative, and idempotent — the whole point of a CRDT.
#[must_use]
pub fn merge_set_member(
    local: &SetMember,
    incoming_is_add: bool,
    incoming_clock: &Hlc,
) -> SetMergeOutcome {
    let mut member = local.clone();
    let slot = if incoming_is_add {
        &mut member.add_clock
    } else {
        &mut member.remove_clock
    };
    match slot {
        Some(existing) if incoming_clock <= existing => {}
        _ => *slot = Some(incoming_clock.clone()),
    }
    let present = member.is_present();
    SetMergeOutcome { member, present }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::panic)]

    use super::*;

    fn hlc(w: u64, c: u32, d: &str) -> Hlc {
        Hlc::new(w, c, d.to_owned())
    }

    fn val(s: &str) -> Value {
        Value::String(s.to_owned())
    }

    // --- strategy table -----------------------------------------------------

    #[test]
    fn strategy_table_matches_adr_023() {
        assert_eq!(
            strategy_for(EntityType::Document, "status"),
            ConflictStrategy::Lww
        );
        assert_eq!(
            strategy_for(EntityType::Document, "body"),
            ConflictStrategy::ConflictCopy
        );
        assert_eq!(
            strategy_for(EntityType::TagEdge, "present"),
            ConflictStrategy::SetUnionTombstone
        );
        assert_eq!(
            strategy_for(EntityType::CollectionEdge, "present"),
            ConflictStrategy::SetUnionTombstone
        );
        assert_eq!(
            strategy_for(EntityType::Note, "body"),
            ConflictStrategy::ConflictCopy
        );
        assert_eq!(
            strategy_for(EntityType::Highlight, "note"),
            ConflictStrategy::ConflictCopy
        );
        assert_eq!(
            strategy_for(EntityType::Highlight, "color"),
            ConflictStrategy::Lww
        );
        assert_eq!(
            strategy_for(EntityType::ReviewCard, "enabled"),
            ConflictStrategy::Lww
        );
        assert_eq!(
            strategy_for(EntityType::ReviewCard, "stability"),
            ConflictStrategy::DerivedMerge
        );
        assert_eq!(
            strategy_for(EntityType::FeedSubscription, "title"),
            ConflictStrategy::Lww
        );
    }

    // --- LWW ----------------------------------------------------------------

    #[test]
    fn lww_takes_greater_clock() {
        let local_clock = hlc(1, 0, "a");
        let local_val = val("old");
        let inc = hlc(2, 0, "a");
        let d = merge_field(
            ConflictStrategy::Lww,
            &FieldMerge {
                local: Some((&local_val, &local_clock)),
                incoming_value: &val("new"),
                incoming_clock: &inc,
                base_version: None,
            },
        );
        assert_eq!(d, MergeDecision::TakeIncoming);
    }

    #[test]
    fn lww_keeps_local_when_incoming_is_older() {
        let local_clock = hlc(5, 0, "a");
        let local_val = val("keep");
        let inc = hlc(2, 0, "b");
        let d = merge_field(
            ConflictStrategy::Lww,
            &FieldMerge {
                local: Some((&local_val, &local_clock)),
                incoming_value: &val("stale"),
                incoming_clock: &inc,
                base_version: None,
            },
        );
        assert_eq!(d, MergeDecision::KeepLocal);
    }

    #[test]
    fn lww_takes_incoming_when_local_absent() {
        let inc = hlc(1, 0, "a");
        let d = merge_field(
            ConflictStrategy::Lww,
            &FieldMerge {
                local: None,
                incoming_value: &val("new"),
                incoming_clock: &inc,
                base_version: None,
            },
        );
        assert_eq!(d, MergeDecision::TakeIncoming);
    }

    #[test]
    fn lww_is_deterministic_on_equal_causal_key() {
        // Equal (wall, counter): device_id tiebreak decides, both directions.
        let local_clock = hlc(3, 1, "aaa");
        let local_val = val("local");
        let inc_hi = hlc(3, 1, "zzz");
        let d = merge_field(
            ConflictStrategy::Lww,
            &FieldMerge {
                local: Some((&local_val, &local_clock)),
                incoming_value: &val("remote"),
                incoming_clock: &inc_hi,
                base_version: None,
            },
        );
        assert_eq!(d, MergeDecision::TakeIncoming);

        let local_hi = hlc(3, 1, "zzz");
        let inc_lo = hlc(3, 1, "aaa");
        let d2 = merge_field(
            ConflictStrategy::Lww,
            &FieldMerge {
                local: Some((&local_val, &local_hi)),
                incoming_value: &val("remote"),
                incoming_clock: &inc_lo,
                base_version: None,
            },
        );
        assert_eq!(d2, MergeDecision::KeepLocal);
    }

    // --- conflict-copy ------------------------------------------------------

    #[test]
    fn conflict_copy_causal_fast_forward_applies() {
        let base = hlc(1, 0, "a");
        let local_val = val("v1");
        let inc = hlc(2, 0, "a");
        let d = merge_field(
            ConflictStrategy::ConflictCopy,
            &FieldMerge {
                local: Some((&local_val, &base)),
                incoming_value: &val("v2"),
                incoming_clock: &inc,
                base_version: Some(&base), // writer saw exactly our version
            },
        );
        assert_eq!(d, MergeDecision::TakeIncoming);
    }

    #[test]
    fn conflict_copy_concurrent_keeps_both_winner_by_hlc() {
        let local_clock = hlc(5, 0, "a");
        let local_val = val("local body");
        let inc = hlc(9, 0, "b");
        let observed = hlc(1, 0, "a"); // writer branched from an older version
        let d = merge_field(
            ConflictStrategy::ConflictCopy,
            &FieldMerge {
                local: Some((&local_val, &local_clock)),
                incoming_value: &val("remote body"),
                incoming_clock: &inc,
                base_version: Some(&observed),
            },
        );
        match d {
            MergeDecision::ConflictCopy {
                winner,
                loser,
                winner_clock,
                loser_clock,
            } => {
                assert_eq!(winner, val("remote body"));
                assert_eq!(loser, val("local body"));
                assert_eq!(winner_clock, inc);
                assert_eq!(loser_clock, local_clock);
            }
            other => panic!("expected conflict copy, got {other:?}"),
        }
    }

    #[test]
    fn conflict_copy_concurrent_local_wins_still_records_loser() {
        let local_clock = hlc(9, 0, "z");
        let local_val = val("local wins");
        let inc = hlc(4, 0, "a");
        let d = merge_field(
            ConflictStrategy::ConflictCopy,
            &FieldMerge {
                local: Some((&local_val, &local_clock)),
                incoming_value: &val("remote loses"),
                incoming_clock: &inc,
                base_version: None, // did not observe our version -> concurrent
            },
        );
        match d {
            MergeDecision::ConflictCopy { winner, loser, .. } => {
                assert_eq!(winner, val("local wins"));
                assert_eq!(loser, val("remote loses"));
            }
            other => panic!("expected conflict copy, got {other:?}"),
        }
    }

    #[test]
    fn conflict_copy_identical_values_never_conflict() {
        let local_clock = hlc(5, 0, "a");
        let local_val = val("same");
        let inc = hlc(9, 0, "b");
        let d = merge_field(
            ConflictStrategy::ConflictCopy,
            &FieldMerge {
                local: Some((&local_val, &local_clock)),
                incoming_value: &val("same"),
                incoming_clock: &inc,
                base_version: None,
            },
        );
        assert_eq!(d, MergeDecision::TakeIncoming);
    }

    // --- set-union / observed-remove ---------------------------------------

    #[test]
    fn set_add_makes_present() {
        let out = merge_set_member(&SetMember::default(), true, &hlc(1, 0, "a"));
        assert!(out.present);
    }

    #[test]
    fn set_concurrent_adds_both_survive() {
        // Two devices add the same edge concurrently — still present, and the
        // greatest add clock is retained.
        let s = merge_set_member(&SetMember::default(), true, &hlc(1, 0, "a")).member;
        let out = merge_set_member(&s, true, &hlc(1, 0, "b"));
        assert!(out.present);
        assert_eq!(out.member.add_clock, Some(hlc(1, 0, "b")));
    }

    #[test]
    fn set_observed_remove_dominates_earlier_add() {
        let s = merge_set_member(&SetMember::default(), true, &hlc(1, 0, "a")).member;
        let out = merge_set_member(&s, false, &hlc(2, 0, "a"));
        assert!(!out.present);
    }

    #[test]
    fn set_concurrent_add_survives_unseen_remove() {
        // remove at (1,"a") did not observe a concurrent add at (2,"b").
        let mut s = merge_set_member(&SetMember::default(), true, &hlc(1, 0, "a")).member;
        s = merge_set_member(&s, false, &hlc(1, 0, "a")).member; // self remove (won't dominate later add)
        let out = merge_set_member(&s, true, &hlc(2, 0, "b"));
        assert!(out.present, "a later add resurrects the edge");
    }

    #[test]
    fn set_readd_after_dominated_delete_resurrects() {
        let mut s = merge_set_member(&SetMember::default(), true, &hlc(1, 0, "a")).member;
        s = merge_set_member(&s, false, &hlc(2, 0, "a")).member;
        assert!(!s.is_present());
        let out = merge_set_member(&s, true, &hlc(3, 0, "a"));
        assert!(out.present);
    }

    #[test]
    fn set_merge_is_commutative() {
        let add = hlc(1, 0, "a");
        let remove = hlc(2, 0, "b");
        // add then remove
        let mut a = merge_set_member(&SetMember::default(), true, &add).member;
        a = merge_set_member(&a, false, &remove).member;
        // remove then add
        let mut b = merge_set_member(&SetMember::default(), false, &remove).member;
        b = merge_set_member(&b, true, &add).member;
        assert_eq!(a, b);
        assert_eq!(a.is_present(), b.is_present());
    }

    #[test]
    fn set_merge_is_idempotent() {
        let add = hlc(1, 0, "a");
        let once = merge_set_member(&SetMember::default(), true, &add).member;
        let twice = merge_set_member(&once, true, &add).member;
        assert_eq!(once, twice);
    }
}
