//! The hybrid logical clock (HLC) — the ordering primitive for sync (ADR-022 /
//! ADR-023).
//!
//! An HLC stamp is `{wall_millis, counter, device_id}`. It gives a **total,
//! causally-aware order** without trusting wall clocks: the `counter` advances
//! monotonically even when two events share a wall-clock millisecond or arrive
//! out of order, and the `device_id` breaks genuine ties **deterministically**
//! so every device folds the same event set into the same state regardless of
//! pull order (ADR-023 "Ordering primitive").
//!
//! Comparison order is `(wall_millis, counter, device_id)`, lexicographically.
//! Ties on `(wall_millis, counter)` break on the **larger** `device_id`, which
//! is a total order, not a coin flip.

use serde::{Deserialize, Serialize};

/// A hybrid logical clock stamp attached to every change.
///
/// The pair `(wall_millis, counter)` is the causal component; `device_id` is
/// the deterministic tiebreak that makes the order total. Two stamps are
/// **concurrent** in the causal sense when neither was produced with knowledge
/// of the other — the sync engine detects that via the *observed prior version*
/// carried on a change (see [`crate::sync::event`]), not by comparing stamps
/// alone. This type only provides the total order and the tick/observe update
/// rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hlc {
    /// Best-effort wall-clock time in epoch milliseconds. Never trusted on its
    /// own for ordering; the counter and `device_id` disambiguate skew.
    pub wall_millis: u64,
    /// Logical counter, advanced when the wall clock does not move forward, so
    /// distinct events on one device always get distinct, ordered stamps.
    pub counter: u32,
    /// Opaque origin-device handle; the deterministic final tiebreak.
    pub device_id: String,
}

impl Hlc {
    /// Create a stamp from its parts.
    #[must_use]
    pub const fn new(wall_millis: u64, counter: u32, device_id: String) -> Self {
        Self {
            wall_millis,
            counter,
            device_id,
        }
    }

    /// The causal key `(wall_millis, counter)` used for ordering before the
    /// device tiebreak.
    #[must_use]
    pub const fn causal_key(&self) -> (u64, u32) {
        (self.wall_millis, self.counter)
    }

    /// Advance this device's local clock for a **new local event** at the given
    /// wall-clock reading, returning the stamp to attach to that event.
    ///
    /// If physical time moved forward past our last stamp we adopt it and reset
    /// the counter; otherwise we keep the last wall time and bump the counter,
    /// guaranteeing the new stamp is strictly greater than the previous one.
    #[must_use]
    pub fn tick(&self, now_millis: u64) -> Self {
        if now_millis > self.wall_millis {
            Self::new(now_millis, 0, self.device_id.clone())
        } else {
            Self::new(
                self.wall_millis,
                self.counter.saturating_add(1),
                self.device_id.clone(),
            )
        }
    }

    /// Advance this device's local clock on **receiving** a remote stamp,
    /// returning the updated local clock (Lamport-style HLC receive rule).
    ///
    /// The new wall time is the max of ours, the remote's, and the physical
    /// reading; the counter is bumped so the local clock never goes backwards
    /// and stays strictly ahead of anything it has observed.
    #[must_use]
    pub fn observe(&self, remote: &Self, now_millis: u64) -> Self {
        let wall = self.wall_millis.max(remote.wall_millis).max(now_millis);
        let counter = if wall == self.wall_millis && wall == remote.wall_millis {
            self.counter.max(remote.counter).saturating_add(1)
        } else if wall == self.wall_millis {
            self.counter.saturating_add(1)
        } else if wall == remote.wall_millis {
            remote.counter.saturating_add(1)
        } else {
            0
        };
        Self::new(wall, counter, self.device_id.clone())
    }

    /// The initial clock for a device that has never emitted an event.
    #[must_use]
    pub const fn zero(device_id: String) -> Self {
        Self::new(0, 0, device_id)
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.causal_key()
            .cmp(&other.causal_key())
            .then_with(|| self.device_id.cmp(&other.device_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hlc(w: u64, c: u32, d: &str) -> Hlc {
        Hlc::new(w, c, d.to_owned())
    }

    #[test]
    fn causal_key_orders_before_device_tiebreak() {
        assert!(hlc(1, 0, "z") < hlc(1, 1, "a"));
        assert!(hlc(1, 0, "z") < hlc(2, 0, "a"));
    }

    #[test]
    fn device_id_breaks_ties_deterministically() {
        // Equal causal key: larger device_id wins the total order.
        assert!(hlc(5, 3, "aaa") < hlc(5, 3, "bbb"));
        assert_eq!(hlc(5, 3, "x"), hlc(5, 3, "x"));
    }

    #[test]
    fn tick_advances_counter_within_same_millis() {
        let clock = hlc(100, 0, "dev");
        let next = clock.tick(100);
        assert_eq!(next.wall_millis, 100);
        assert_eq!(next.counter, 1);
        assert!(clock < next);
    }

    #[test]
    fn tick_adopts_forward_wall_time_and_resets_counter() {
        let clock = hlc(100, 7, "dev");
        let next = clock.tick(250);
        assert_eq!(next.wall_millis, 250);
        assert_eq!(next.counter, 0);
        assert!(clock < next);
    }

    #[test]
    fn tick_is_strictly_monotonic_under_stalled_clock() {
        let mut clock = hlc(0, 0, "dev");
        let mut prev = clock.clone();
        for _ in 0..1000 {
            clock = clock.tick(50); // physical clock stuck at 50ms
            assert!(prev < clock);
            prev = clock.clone();
        }
    }

    #[test]
    fn observe_stays_ahead_of_remote() {
        let local = hlc(100, 2, "dev");
        let remote = hlc(140, 9, "other");
        let updated = local.observe(&remote, 130);
        assert_eq!(updated.wall_millis, 140);
        assert_eq!(updated.counter, 10);
        assert!(updated > remote);
        assert!(updated > local);
    }

    #[test]
    fn observe_uses_physical_time_when_it_leads() {
        let local = hlc(100, 2, "dev");
        let remote = hlc(90, 1, "other");
        let updated = local.observe(&remote, 200);
        assert_eq!(updated.wall_millis, 200);
        assert_eq!(updated.counter, 0);
    }

    #[test]
    fn observe_bumps_counter_on_equal_wall_times() {
        let local = hlc(100, 4, "dev");
        let remote = hlc(100, 9, "other");
        let updated = local.observe(&remote, 80);
        assert_eq!(updated.wall_millis, 100);
        assert_eq!(updated.counter, 10);
    }

    #[test]
    fn total_order_is_consistent_regardless_of_input_order() {
        let mut a = vec![
            hlc(3, 1, "b"),
            hlc(1, 0, "z"),
            hlc(3, 1, "a"),
            hlc(2, 5, "m"),
        ];
        let mut b = a.clone();
        b.reverse();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }
}
