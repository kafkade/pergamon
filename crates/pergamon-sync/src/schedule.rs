// SPDX-License-Identifier: Apache-2.0

//! Scheduling policy for *background* sync (issue #129).
//!
//! The [`SyncEngine`](crate::engine::SyncEngine) performs one-shot rounds; a
//! background driver must decide **when to run the next round** and how to
//! behave when the network is unavailable. That decision logic lives here as a
//! pure, clock-free, I/O-free state machine so it can be unit-tested exhaustively
//! and shared by every platform (CLI daemon, web-server worker, iOS background
//! refresh):
//!
//! - [`BackoffPolicy`] — exponential backoff with equal jitter, capped, computed
//!   from a caller-supplied random fraction (no RNG dependency in core).
//! - [`SyncScheduler`] — tracks the consecutive-failure count and yields the
//!   delay before the next attempt: the steady-state `interval` after a success,
//!   or a growing backoff after a retryable/offline failure.
//!
//! The blocking loop that ties this to real time and a real transport lives in
//! [`crate::daemon`].

use std::time::Duration;

/// Exponential backoff with equal jitter.
///
/// Given a zero-based retry number, the *ceiling* delay is
/// `base * multiplier^retry`, clamped to `max`. Equal jitter then keeps the
/// actual delay in `[ceiling/2, ceiling]`, so retries neither synchronize into
/// thundering herds nor collapse to near-zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackoffPolicy {
    /// Delay for the first retry, before any exponential growth.
    base: Duration,
    /// Upper bound the ceiling delay is clamped to.
    max: Duration,
    /// Growth factor applied per additional consecutive failure (`>= 1.0`).
    multiplier: f64,
}

impl BackoffPolicy {
    /// Build a policy, clamping `multiplier` to at least `1.0` and ensuring
    /// `max >= base`.
    #[must_use]
    pub fn new(base: Duration, max: Duration, multiplier: f64) -> Self {
        let multiplier = if multiplier.is_finite() && multiplier >= 1.0 {
            multiplier
        } else {
            1.0
        };
        let max = if max >= base { max } else { base };
        Self {
            base,
            max,
            multiplier,
        }
    }

    /// The base (first-retry) delay.
    #[must_use]
    pub const fn base(&self) -> Duration {
        self.base
    }

    /// The maximum delay ceiling.
    #[must_use]
    pub const fn max(&self) -> Duration {
        self.max
    }

    /// The uncapped-then-capped ceiling delay for a zero-based `retry` number,
    /// before jitter is applied.
    #[must_use]
    pub fn ceiling(&self, retry: u32) -> Duration {
        let factor = self
            .multiplier
            .powi(i32::try_from(retry).unwrap_or(i32::MAX));
        let secs = self.base.as_secs_f64() * factor;
        if !secs.is_finite() || secs >= self.max.as_secs_f64() {
            self.max
        } else {
            Duration::from_secs_f64(secs)
        }
    }

    /// The jittered delay for a zero-based `retry` number.
    ///
    /// `rand01` is a caller-supplied fraction in `[0.0, 1.0)`; values outside
    /// that range are clamped. The result lies in `[ceiling/2, ceiling]`.
    #[must_use]
    pub fn delay(&self, retry: u32, rand01: f64) -> Duration {
        let rand01 = rand01.clamp(0.0, 1.0);
        let ceiling = self.ceiling(retry).as_secs_f64();
        let half = ceiling / 2.0;
        Duration::from_secs_f64(half + half * rand01)
    }
}

impl Default for BackoffPolicy {
    /// A sensible default: 5s base, 5min ceiling, doubling.
    #[allow(clippy::duration_suboptimal_units)] // explicit seconds read clearly here
    fn default() -> Self {
        Self::new(Duration::from_secs(5), Duration::from_secs(300), 2.0)
    }
}

/// Decides the delay before the next background sync round.
///
/// After a successful round it returns the steady-state `interval`; after a
/// retryable (offline/transient) failure it returns a growing [`BackoffPolicy`]
/// delay driven by the consecutive-failure count, which resets on the next
/// success.
#[derive(Debug, Clone)]
pub struct SyncScheduler {
    interval: Duration,
    backoff: BackoffPolicy,
    consecutive_failures: u32,
}

impl SyncScheduler {
    /// Build a scheduler with a steady-state `interval` and a `backoff` policy
    /// used while failing.
    #[must_use]
    pub const fn new(interval: Duration, backoff: BackoffPolicy) -> Self {
        Self {
            interval,
            backoff,
            consecutive_failures: 0,
        }
    }

    /// The steady-state interval between successful rounds.
    #[must_use]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    /// The current run of consecutive retryable failures (0 when healthy).
    #[must_use]
    pub const fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Whether the scheduler is currently in a backoff (failing) state.
    #[must_use]
    pub const fn is_backing_off(&self) -> bool {
        self.consecutive_failures > 0
    }

    /// Record a successful round, clearing any backoff.
    pub const fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Record a retryable/offline failure, deepening the backoff.
    pub const fn record_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
    }

    /// The delay before the next round, given a random fraction `rand01` in
    /// `[0.0, 1.0)` used only while backing off.
    #[must_use]
    pub fn next_delay(&self, rand01: f64) -> Duration {
        if self.consecutive_failures == 0 {
            self.interval
        } else {
            self.backoff.delay(self.consecutive_failures - 1, rand01)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::float_cmp)]
    #![allow(clippy::duration_suboptimal_units)]

    use super::*;

    #[test]
    fn ceiling_grows_then_caps() {
        let p = BackoffPolicy::new(Duration::from_secs(1), Duration::from_secs(30), 2.0);
        assert_eq!(p.ceiling(0), Duration::from_secs(1));
        assert_eq!(p.ceiling(1), Duration::from_secs(2));
        assert_eq!(p.ceiling(2), Duration::from_secs(4));
        assert_eq!(p.ceiling(3), Duration::from_secs(8));
        assert_eq!(p.ceiling(4), Duration::from_secs(16));
        // 32 > 30 cap → clamped.
        assert_eq!(p.ceiling(5), Duration::from_secs(30));
        assert_eq!(p.ceiling(100), Duration::from_secs(30));
    }

    #[test]
    fn delay_stays_within_equal_jitter_band() {
        let p = BackoffPolicy::new(Duration::from_secs(8), Duration::from_secs(600), 2.0);
        let ceiling = p.ceiling(2); // 32s
        // rand01 = 0 → half the ceiling; rand01 → 1 → the full ceiling.
        assert_eq!(p.delay(2, 0.0), ceiling / 2);
        assert_eq!(p.delay(2, 1.0), ceiling);
        let mid = p.delay(2, 0.5);
        assert!(mid > ceiling / 2 && mid < ceiling);
    }

    #[test]
    fn delay_clamps_out_of_range_rand() {
        let p = BackoffPolicy::default();
        assert_eq!(p.delay(0, -5.0), p.delay(0, 0.0));
        assert_eq!(p.delay(0, 5.0), p.delay(0, 1.0));
    }

    #[test]
    fn invalid_multiplier_and_max_are_sanitized() {
        let p = BackoffPolicy::new(Duration::from_secs(10), Duration::from_secs(1), f64::NAN);
        // multiplier clamped to 1.0 → no growth.
        assert_eq!(p.ceiling(0), p.ceiling(5));
        // max clamped up to base.
        assert_eq!(p.max(), Duration::from_secs(10));
    }

    #[test]
    fn scheduler_returns_interval_when_healthy() {
        let s = SyncScheduler::new(Duration::from_secs(60), BackoffPolicy::default());
        assert!(!s.is_backing_off());
        assert_eq!(s.next_delay(0.5), Duration::from_secs(60));
    }

    #[test]
    fn scheduler_backs_off_after_failures_and_recovers() {
        let backoff = BackoffPolicy::new(Duration::from_secs(2), Duration::from_secs(60), 2.0);
        let mut s = SyncScheduler::new(Duration::from_secs(300), backoff);

        s.record_failure();
        assert_eq!(s.consecutive_failures(), 1);
        // first failure → retry 0 → ceiling 2s, full jitter → 2s.
        assert_eq!(s.next_delay(1.0), Duration::from_secs(2));

        s.record_failure();
        // second failure → retry 1 → ceiling 4s.
        assert_eq!(s.next_delay(1.0), Duration::from_secs(4));

        s.record_success();
        assert!(!s.is_backing_off());
        assert_eq!(s.next_delay(0.5), Duration::from_secs(300));
    }
}
