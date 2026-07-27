// SPDX-License-Identifier: AGPL-3.0-only

//! Per-identity online-guessing throttle (design §1.7).
//!
//! Because OPAQUE's OPRF makes *offline* guessing expensive, the dominant
//! residual risk is *online* guessing. This module owns the **per-identity**
//! half of the layered defense: an exponential backoff / lockout keyed on the
//! `identity_handle` and driven by a failure counter in
//! [`crate::auth::store::AuthStore`].
//!
//! ## No existence oracle
//! Failure counters are keyed on the `identity_handle` **uniformly**, whether or
//! not an account exists for it. A lockout therefore reflects only the handle's
//! own recent failure history, never whether it is registered — so throttling
//! cannot be turned into an account-existence oracle (design §1.6).
//!
//! ## Escalating delay, not permanent lockout
//! We apply an escalating delay rather than a hard permanent lock, to avoid a
//! trivial account-denial vector (design §1.7). The delay grows exponentially
//! with the failure count past a threshold, capped at a maximum.
//!
//! ## Scope seam — WP-4 / #195
//! This is only the per-identity control. The transport-level controls that sit
//! in front of every route — per-IP / per-subnet rate limiting, body caps, and
//! storage-DoS isolation — are **out of scope here** and owned by WP-4
//! ([#195](https://github.com/kafkade/pergamon/issues/195)). Those also bound
//! the growth of the failure table under handle-spraying, which this per-identity
//! counter cannot do alone.

// `Duration::from_secs(15 * 60)` is deliberate: the more-granular constructors
// `Duration::from_mins`/`from_hours` that clippy's `duration_suboptimal_units`
// suggests are still unstable on our MSRV (1.95), so we keep `from_secs` and
// silence that (nursery) lint here.
#![allow(clippy::duration_suboptimal_units)]

use std::time::Duration;

/// Configuration for the per-identity backoff/lockout policy.
#[derive(Debug, Clone, Copy)]
pub struct ThrottleConfig {
    /// Number of consecutive failures tolerated before any lockout applies.
    pub threshold: u32,
    /// Base delay applied at the first lockout step.
    pub base_delay: Duration,
    /// Maximum delay any single lockout step may reach.
    pub max_delay: Duration,
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self {
            // Allow a few honest typos, then start escalating.
            threshold: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(15 * 60),
        }
    }
}

impl ThrottleConfig {
    /// Compute the lockout duration for a running count of consecutive failures.
    ///
    /// Returns `Duration::ZERO` while `failures <= threshold`. Past the
    /// threshold the delay is `base_delay * 2^(failures - threshold - 1)`,
    /// saturating at `max_delay`. The exponent is computed with saturating math
    /// so a very large failure count cannot overflow.
    #[must_use]
    pub fn lockout_for(&self, failures: u32) -> Duration {
        if failures <= self.threshold {
            return Duration::ZERO;
        }
        let steps = failures - self.threshold - 1;
        // Saturate the shift: anything past ~40 steps already exceeds max_delay.
        let factor: u64 = 1u64.checked_shl(steps.min(40)).unwrap_or(u64::MAX);
        let base_ms = u64::try_from(self.base_delay.as_millis().min(u128::from(u64::MAX)))
            .unwrap_or(u64::MAX);
        let delay_ms = base_ms.saturating_mul(factor);
        let max_ms =
            u64::try_from(self.max_delay.as_millis().min(u128::from(u64::MAX))).unwrap_or(u64::MAX);
        Duration::from_millis(delay_ms.min(max_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_lockout_at_or_below_threshold() {
        let cfg = ThrottleConfig::default();
        for failures in 0..=cfg.threshold {
            assert_eq!(cfg.lockout_for(failures), Duration::ZERO);
        }
    }

    #[test]
    fn lockout_grows_exponentially_then_caps() {
        let cfg = ThrottleConfig {
            threshold: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
        };
        assert_eq!(cfg.lockout_for(4), Duration::from_secs(1)); // 2^0
        assert_eq!(cfg.lockout_for(5), Duration::from_secs(2)); // 2^1
        assert_eq!(cfg.lockout_for(6), Duration::from_secs(4)); // 2^2
        assert_eq!(cfg.lockout_for(7), Duration::from_secs(8)); // 2^3
        // 2^6 = 64s would exceed the 60s cap.
        assert_eq!(cfg.lockout_for(10), Duration::from_secs(60));
    }

    #[test]
    fn extreme_failure_count_saturates_without_panic() {
        let cfg = ThrottleConfig::default();
        // Must not overflow/panic and must not exceed max_delay.
        assert_eq!(cfg.lockout_for(u32::MAX), cfg.max_delay);
    }
}
