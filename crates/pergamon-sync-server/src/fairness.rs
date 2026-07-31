// SPDX-License-Identifier: AGPL-3.0-only

//! Per-tenant fairness for the AGPL sync server (WP-3e, [#201]).
//!
//! **NOT YET EXTERNALLY SECURITY-REVIEWED — do not deploy** without the review
//! that gates the rest of the managed-hosting work packages.
//!
//! ## Why this exists
//! WP-4 ([#195]) added a **global** concurrency limit with load-shedding and
//! per-IP rate limits. Those bound total load and per-source rate, but they
//! deliberately gave no per-tenant *fairness*, because at the time everything
//! contended on one process-wide store mutex anyway; WP-4 named this issue as
//! the place to fix it.
//!
//! WP-3e replaced that mutex with a bounded reader pool, which creates exactly
//! the gap WP-4 predicted: a single heavy tenant can hold **every** pooled
//! connection and stall everyone else. [`TenantLimiter`] closes it by capping
//! how many store operations one `account_id` may have in flight.
//!
//! ## What it guarantees — and what it does not
//! With the default cap of `read_pool_size - 1`, no single tenant can occupy the
//! last reader connection, so a second tenant always has capacity available.
//! That is the honest, narrow guarantee.
//!
//! It is **not** proportional fairness: with many tenants above the cap, the
//! pool is still first-come-first-served among them, and it does not weight
//! tenants by plan, history, or cost. It is also not a rate limit — it bounds
//! *concurrency*, not requests per second (that is WP-4's job) — and it does not
//! change the single-writer ceiling: writes serialize on one connection whatever
//! this limiter does.
//!
//! ## Memory safety of the tenant map
//! In blind mode the server has no auth plane, so `account_id` is unauthenticated,
//! attacker-supplied input. A map keyed on it that only ever grew would be a
//! trivial memory-DoS vector. Entries are therefore removed the moment a
//! tenant's in-flight count returns to zero, so the map's size is bounded by
//! *concurrently active* tenants, never by the number of distinct ids ever seen.
//!
//! [#195]: https://github.com/kafkade/pergamon/issues/195
//! [#201]: https://github.com/kafkade/pergamon/issues/201

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Errors returned when acquiring a per-tenant slot.
#[derive(Debug, thiserror::Error)]
pub enum FairnessError {
    /// The tenant held its maximum in-flight operations for the whole wait.
    #[error(
        "account exceeded its concurrent-request allowance ({cap}); waited {waited_ms}ms for a slot"
    )]
    Busy {
        /// The per-tenant cap that was hit.
        cap: usize,
        /// How long the caller actually waited, in milliseconds.
        waited_ms: u64,
    },

    /// The limiter's internal lock was poisoned by a panic in another thread.
    #[error("tenant limiter lock poisoned")]
    Poisoned,
}

/// Per-tenant concurrency policy.
#[derive(Debug, Clone, Copy)]
pub struct FairnessConfig {
    /// Maximum store operations one `account_id` may have in flight. `0`
    /// disables the limiter entirely.
    pub max_tenant_concurrency: usize,
    /// How long an over-cap caller waits for a slot before being shed.
    pub wait_timeout: Duration,
}

impl FairnessConfig {
    /// The recommended default for a reader pool of `pool_size`.
    ///
    /// `pool_size - 1` (floored at 1): a single tenant can never take the last
    /// connection, so another tenant always gets in, while a single-tenant
    /// self-host is effectively unaffected.
    #[must_use]
    pub const fn for_pool(pool_size: usize, wait_timeout: Duration) -> Self {
        Self {
            max_tenant_concurrency: if pool_size > 1 { pool_size - 1 } else { 1 },
            wait_timeout,
        }
    }

    /// A disabled limiter: every tenant may use the whole pool.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            max_tenant_concurrency: 0,
            wait_timeout: Duration::from_millis(0),
        }
    }

    /// Whether the limiter is switched off.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.max_tenant_concurrency == 0
    }
}

/// Caps how many store operations a single tenant may have in flight.
#[derive(Debug)]
pub struct TenantLimiter {
    /// In-flight count per active `account_id`. Entries are removed at zero.
    inflight: Mutex<HashMap<String, usize>>,
    /// Signalled whenever a tenant slot is released.
    released: Condvar,
    /// The configured policy.
    config: FairnessConfig,
}

impl TenantLimiter {
    /// Build a limiter with the given policy.
    #[must_use]
    pub fn new(config: FairnessConfig) -> Self {
        Self {
            inflight: Mutex::new(HashMap::new()),
            released: Condvar::new(),
            config,
        }
    }

    /// The configured policy.
    #[must_use]
    pub const fn config(&self) -> FairnessConfig {
        self.config
    }

    /// Number of tenants currently holding at least one slot.
    ///
    /// Used by tests to prove the map is pruned rather than growing without
    /// bound on attacker-supplied `account_id`s.
    ///
    /// # Errors
    /// Returns [`FairnessError::Poisoned`] if the internal lock is poisoned.
    pub fn active_tenants(&self) -> Result<usize, FairnessError> {
        Ok(self
            .inflight
            .lock()
            .map_err(|_| FairnessError::Poisoned)?
            .len())
    }

    /// Acquire a slot for `account_id`, blocking until one is free or the
    /// configured wait expires.
    ///
    /// Returns `None` when the limiter is disabled — callers proceed unguarded,
    /// which keeps the disabled path free of any bookkeeping.
    ///
    /// This blocks the calling thread, so callers on an async runtime must run
    /// it inside `tokio::task::spawn_blocking`.
    ///
    /// # Errors
    /// Returns [`FairnessError::Busy`] if the tenant stayed at its cap for the
    /// whole wait, or [`FairnessError::Poisoned`] if the lock is poisoned.
    pub fn acquire(
        self: &Arc<Self>,
        account_id: &str,
    ) -> Result<Option<TenantSlot>, FairnessError> {
        if self.config.is_disabled() {
            return Ok(None);
        }
        let cap = self.config.max_tenant_concurrency;
        let started = Instant::now();
        let mut inflight = self.inflight.lock().map_err(|_| FairnessError::Poisoned)?;
        loop {
            let current = inflight.get(account_id).copied().unwrap_or(0);
            if current < cap {
                inflight.insert(account_id.to_owned(), current + 1);
                return Ok(Some(TenantSlot {
                    limiter: Arc::clone(self),
                    account_id: account_id.to_owned(),
                }));
            }
            // `Condvar` wakeups can be spurious, so re-check against a deadline.
            let Some(remaining) = self.config.wait_timeout.checked_sub(started.elapsed()) else {
                return Err(FairnessError::Busy {
                    cap,
                    waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                });
            };
            let (guard, timed_out) = self
                .released
                .wait_timeout(inflight, remaining)
                .map_err(|_| FairnessError::Poisoned)?;
            inflight = guard;
            if timed_out.timed_out() && inflight.get(account_id).copied().unwrap_or(0) >= cap {
                return Err(FairnessError::Busy {
                    cap,
                    waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                });
            }
        }
    }

    /// Release one slot for `account_id`, pruning the entry at zero.
    fn release(&self, account_id: &str) {
        if let Ok(mut inflight) = self.inflight.lock() {
            if let Some(count) = inflight.get_mut(account_id) {
                *count -= 1;
                if *count == 0 {
                    // Prune: the map must be bounded by *active* tenants, not by
                    // every account_id an unauthenticated caller ever invented.
                    inflight.remove(account_id);
                }
            }
            drop(inflight);
            // Wake every waiter: a notify_one could wake a thread waiting on a
            // different tenant, which would leave the right one asleep.
            self.released.notify_all();
        }
    }
}

/// A held per-tenant slot, released on drop.
#[derive(Debug)]
pub struct TenantSlot {
    /// The limiter to release back into.
    limiter: Arc<TenantLimiter>,
    /// The tenant this slot belongs to.
    account_id: String,
}

impl Drop for TenantSlot {
    fn drop(&mut self) {
        self.limiter.release(&self.account_id);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]
    #![allow(clippy::panic)]

    use std::thread;

    use super::*;

    fn limiter(cap: usize, wait_ms: u64) -> Arc<TenantLimiter> {
        Arc::new(TenantLimiter::new(FairnessConfig {
            max_tenant_concurrency: cap,
            wait_timeout: Duration::from_millis(wait_ms),
        }))
    }

    #[test]
    fn default_cap_leaves_one_connection_for_other_tenants() {
        let cfg = FairnessConfig::for_pool(8, Duration::from_millis(10));
        assert_eq!(cfg.max_tenant_concurrency, 7);
        // A degenerate single-connection pool still admits one op.
        assert_eq!(
            FairnessConfig::for_pool(1, Duration::from_millis(10)).max_tenant_concurrency,
            1
        );
    }

    #[test]
    fn disabled_limiter_hands_out_no_slots_and_never_blocks() {
        let limiter = Arc::new(TenantLimiter::new(FairnessConfig::disabled()));
        for _ in 0..100 {
            assert!(limiter.acquire("acct").unwrap().is_none());
        }
        assert_eq!(limiter.active_tenants().unwrap(), 0);
    }

    #[test]
    fn a_tenant_is_capped_but_another_tenant_still_gets_in() {
        let limiter = limiter(2, 50);
        let _a1 = limiter.acquire("heavy").unwrap().unwrap();
        let _a2 = limiter.acquire("heavy").unwrap().unwrap();

        // The heavy tenant is at its cap and is shed.
        let err = limiter.acquire("heavy").unwrap_err();
        assert!(matches!(err, FairnessError::Busy { cap: 2, .. }), "{err:?}");

        // A different tenant is unaffected — the point of the whole module.
        assert!(limiter.acquire("quiet").unwrap().is_some());
    }

    #[test]
    fn releasing_a_slot_admits_the_next_caller() {
        let limiter = limiter(1, 50);
        let slot = limiter.acquire("acct").unwrap().unwrap();
        assert!(limiter.acquire("acct").is_err());
        drop(slot);
        assert!(limiter.acquire("acct").unwrap().is_some());
    }

    /// A blocked caller must be woken by a release, not left to time out.
    #[test]
    fn a_released_slot_wakes_a_waiter() {
        let limiter = limiter(1, 10_000);
        let held = limiter.acquire("acct").unwrap().unwrap();

        let waiter = {
            let limiter = Arc::clone(&limiter);
            thread::spawn(move || {
                let started = Instant::now();
                let slot = limiter.acquire("acct").unwrap();
                assert!(slot.is_some());
                started.elapsed()
            })
        };

        thread::sleep(Duration::from_millis(50));
        drop(held);

        let elapsed = waiter.join().unwrap();
        assert!(
            elapsed < Duration::from_secs(5),
            "waiter should have been woken by the release, waited {elapsed:?}"
        );
    }

    /// The map must be bounded by *active* tenants. In blind mode `account_id`
    /// is unauthenticated input, so an unpruned map would be a memory-DoS.
    #[test]
    fn the_tenant_map_is_pruned_and_does_not_grow_without_bound() {
        let limiter = limiter(4, 50);
        for i in 0..10_000 {
            let slot = limiter.acquire(&format!("attacker-{i}")).unwrap();
            assert!(slot.is_some());
            drop(slot);
        }
        assert_eq!(
            limiter.active_tenants().unwrap(),
            0,
            "every finished tenant must be pruned from the map"
        );

        // Only concurrently-held tenants occupy the map.
        let held: Vec<_> = (0..3)
            .map(|i| limiter.acquire(&format!("live-{i}")).unwrap().unwrap())
            .collect();
        assert_eq!(limiter.active_tenants().unwrap(), 3);
        drop(held);
        assert_eq!(limiter.active_tenants().unwrap(), 0);
    }

    /// A panic while holding a slot must not leak the slot.
    #[test]
    fn a_panicking_holder_still_releases_its_slot() {
        let limiter = limiter(1, 100);
        let panicked = {
            let limiter = Arc::clone(&limiter);
            thread::spawn(move || {
                let _slot = limiter.acquire("acct").unwrap();
                panic!("boom");
            })
        };
        assert!(panicked.join().is_err());
        let slot = limiter.acquire("acct").unwrap();
        assert!(
            slot.is_some(),
            "the slot should have been released during unwind"
        );
        assert_eq!(limiter.active_tenants().unwrap(), 1);
        drop(slot);
        assert_eq!(limiter.active_tenants().unwrap(), 0);
    }

    /// Concurrent tenants must genuinely run side by side up to the cap.
    #[test]
    fn tenants_run_concurrently_up_to_the_cap() {
        const TENANTS: usize = 4;
        const PER_TENANT: usize = 3;
        let limiter = limiter(PER_TENANT, 5_000);
        let barrier = Arc::new(std::sync::Barrier::new(TENANTS * PER_TENANT));

        let handles: Vec<_> = (0..TENANTS)
            .flat_map(|t| {
                (0..PER_TENANT).map({
                    let limiter = Arc::clone(&limiter);
                    let barrier = Arc::clone(&barrier);
                    move |_| {
                        let limiter = Arc::clone(&limiter);
                        let barrier = Arc::clone(&barrier);
                        thread::spawn(move || {
                            let slot = limiter.acquire(&format!("tenant-{t}")).unwrap();
                            assert!(slot.is_some());
                            barrier.wait();
                            drop(slot);
                        })
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(limiter.active_tenants().unwrap(), 0);
    }
}
