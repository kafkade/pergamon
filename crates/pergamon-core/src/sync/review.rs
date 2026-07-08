//! ADR-023 **derived-merge** for review scheduling state.
//!
//! Naïve last-writer-wins on a review card's FSRS scheduling scalars
//! (`stability`, `difficulty`, `due_at`, `review_count`, `lapse_count`, …) is
//! actively harmful: two devices that both review the same card would keep only
//! one review and corrupt the counts. ADR-023 resolves this by splitting the
//! card into its **append-only history** (the review logs, which always
//! auto-merge by id) and its **derived schedule**, which is *recomputed* by
//! folding the time-ordered union of all review logs through the deterministic
//! FSRS scheduler (ADR-005).
//!
//! Because the fold is a pure, order-dependent function of the log union, every
//! device that holds the same set of logs derives byte-identical card state —
//! no count doubles, none are lost, and `due_at` is consistent regardless of
//! pull order. This module is that pure fold; the storage/apply layers read the
//! logs and persist the result.

use crate::fsrs::{CardState, MemoryState, Rating, Scheduler};

/// Milliseconds in one day, used to convert review timestamps to the
/// `elapsed_days` the FSRS scheduler consumes.
const MILLIS_PER_DAY: f64 = 86_400_000.0;

/// One review event in a card's append-only history.
///
/// Reduced to just what the derived-merge fold needs: *when* it happened, *what*
/// the user rated, and a stable id that breaks ties between reviews at the same
/// instant.
///
/// The id makes the fold order **total and deterministic**: two devices'
/// reviews that share a wall-clock millisecond are ordered by id identically
/// everywhere, so the derived schedule converges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLogEntry {
    /// Stable, globally-unique id of the review log (its `entity_id`). Also the
    /// idempotency key: duplicate logs with the same id fold in once.
    pub id: String,
    /// When the review happened, in epoch milliseconds. The primary sort key.
    pub reviewed_at_ms: i64,
    /// The rating the user gave.
    pub rating: Rating,
}

/// The scheduling state derived by folding a card's whole review-log union.
///
/// This is what the apply layer writes onto the `review_cards` row, overriding
/// any value-merged scalars. A card with no logs yet derives the `New` default
/// (all `None`/zero), letting a metadata-first sync show a provisional value
/// until the logs arrive.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedCard {
    /// FSRS lifecycle state after the last review.
    pub state: CardState,
    /// Memory stability after the last review, or `None` if never reviewed.
    pub stability: Option<f64>,
    /// Memory difficulty after the last review, or `None` if never reviewed.
    pub difficulty: Option<f64>,
    /// When the most recent review happened (epoch millis), if any.
    pub last_reviewed_at_ms: Option<i64>,
    /// When the card next falls due (epoch millis), if ever reviewed.
    pub due_at_ms: Option<i64>,
    /// Interval in days scheduled by the last review, if any.
    pub scheduled_days: Option<f64>,
    /// Total number of reviews (the size of the log union).
    pub review_count: u32,
    /// Number of `Again` ratings (lapses).
    pub lapse_count: u32,
}

impl Default for DerivedCard {
    fn default() -> Self {
        Self {
            state: CardState::New,
            stability: None,
            difficulty: None,
            last_reviewed_at_ms: None,
            due_at_ms: None,
            scheduled_days: None,
            review_count: 0,
            lapse_count: 0,
        }
    }
}

/// Fold a card's review-log union into its derived scheduling state (ADR-023).
///
/// The logs are ordered by `(reviewed_at_ms, id)` and de-duplicated by id, then
/// replayed through the FSRS `scheduler`, recomputing `elapsed_days` from the
/// gap between consecutive reviews (never trusting a single device's stored
/// elapsed, which was relative to *its* prior review). The result is identical
/// on every device holding the same logs — the convergence property that lets
/// review state merge without a conflict-inbox entry.
#[must_use]
pub fn derive_card_state(scheduler: &Scheduler, logs: &[ReviewLogEntry]) -> DerivedCard {
    // Total, deterministic order: by review time, then by id to break ties.
    let mut ordered: Vec<&ReviewLogEntry> = logs.iter().collect();
    ordered.sort_by(|a, b| {
        a.reviewed_at_ms
            .cmp(&b.reviewed_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut derived = DerivedCard::default();
    let mut memory: Option<MemoryState> = None;
    let mut state = CardState::New;
    let mut last_ms: Option<i64> = None;
    let mut last_id: Option<&str> = None;

    for entry in ordered {
        // Idempotency: a log delivered twice (same id) folds in once.
        if last_id == Some(entry.id.as_str()) && last_ms == Some(entry.reviewed_at_ms) {
            continue;
        }
        let elapsed_days = last_ms.map_or(0.0, |prev| {
            #[allow(clippy::cast_precision_loss)]
            let delta_ms = entry.reviewed_at_ms.saturating_sub(prev) as f64;
            (delta_ms / MILLIS_PER_DAY).max(0.0)
        });

        let output = scheduler.schedule(state, memory, elapsed_days, entry.rating);
        state = output.next_state;
        memory = Some(output.memory);
        derived.scheduled_days = Some(output.scheduled_days);
        #[allow(clippy::cast_possible_truncation)]
        let due_delta_ms = (output.scheduled_days * MILLIS_PER_DAY) as i64;
        derived.due_at_ms = Some(entry.reviewed_at_ms.saturating_add(due_delta_ms));
        derived.review_count += 1;
        if entry.rating == Rating::Again {
            derived.lapse_count += 1;
        }
        last_ms = Some(entry.reviewed_at_ms);
        last_id = Some(entry.id.as_str());
    }

    derived.state = state;
    derived.stability = memory.map(|m| m.stability);
    derived.difficulty = memory.map(|m| m.difficulty);
    derived.last_reviewed_at_ms = last_ms;
    derived
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::float_cmp)]

    use super::*;

    fn log(id: &str, ms: i64, rating: Rating) -> ReviewLogEntry {
        ReviewLogEntry {
            id: id.to_owned(),
            reviewed_at_ms: ms,
            rating,
        }
    }

    fn scheduler() -> Scheduler {
        Scheduler::default_v5()
    }

    #[test]
    fn empty_log_derives_new_card() {
        let d = derive_card_state(&scheduler(), &[]);
        assert_eq!(d, DerivedCard::default());
        assert_eq!(d.state, CardState::New);
        assert_eq!(d.review_count, 0);
        assert!(d.stability.is_none());
    }

    #[test]
    fn single_review_counts_once() {
        let d = derive_card_state(&scheduler(), &[log("l1", 1_000, Rating::Good)]);
        assert_eq!(d.review_count, 1);
        assert_eq!(d.lapse_count, 0);
        assert!(d.stability.is_some());
        assert!(d.due_at_ms.unwrap() > 1_000);
    }

    #[test]
    fn again_rating_counts_as_lapse() {
        let d = derive_card_state(
            &scheduler(),
            &[
                log("l1", 1_000, Rating::Good),
                log("l2", 1_000 + 3 * 86_400_000, Rating::Again),
            ],
        );
        assert_eq!(d.review_count, 2);
        assert_eq!(d.lapse_count, 1);
    }

    #[test]
    fn fold_is_order_independent() {
        // The same log union in different input orders must derive identically —
        // this is the cross-device convergence property.
        let a = log("a", 1_000, Rating::Good);
        let b = log("b", 1_000 + 5 * 86_400_000, Rating::Hard);
        let c = log("c", 1_000 + 12 * 86_400_000, Rating::Easy);
        let forward = derive_card_state(&scheduler(), &[a.clone(), b.clone(), c.clone()]);
        let shuffled = derive_card_state(&scheduler(), &[c, a, b]);
        assert_eq!(forward, shuffled);
    }

    #[test]
    fn duplicate_logs_fold_in_once() {
        let a = log("a", 1_000, Rating::Good);
        let b = log("b", 1_000 + 5 * 86_400_000, Rating::Good);
        let once = derive_card_state(&scheduler(), &[a.clone(), b.clone()]);
        let twice = derive_card_state(&scheduler(), &[a.clone(), b.clone(), a, b]);
        assert_eq!(once, twice);
        assert_eq!(twice.review_count, 2);
    }

    #[test]
    fn ties_break_deterministically_on_id() {
        // Two reviews at the same instant: ordering by id must be stable so both
        // devices agree on which rating was "first".
        let x = log("x", 5_000, Rating::Again);
        let y = log("y", 5_000, Rating::Easy);
        let one = derive_card_state(&scheduler(), &[x.clone(), y.clone()]);
        let two = derive_card_state(&scheduler(), &[y, x]);
        assert_eq!(one, two);
        assert_eq!(one.review_count, 2);
    }

    #[test]
    fn union_of_two_devices_reviews_sums_counts() {
        // Device A did 2 reviews, device B did 1 on the same card; the union
        // must reflect all three, never doubling or dropping.
        let a1 = log("a1", 1_000, Rating::Good);
        let a2 = log("a2", 1_000 + 7 * 86_400_000, Rating::Good);
        let b1 = log("b1", 1_000 + 2 * 86_400_000, Rating::Again);
        let d = derive_card_state(&scheduler(), &[a1, a2, b1]);
        assert_eq!(d.review_count, 3);
        assert_eq!(d.lapse_count, 1);
    }
}
