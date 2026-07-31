// SPDX-License-Identifier: AGPL-3.0-only

//! Bounded `SQLite` reader-connection pool for the AGPL sync server (WP-3e, [#201]).
//!
//! **NOT YET EXTERNALLY SECURITY-REVIEWED — do not deploy** without the review
//! that gates the rest of the managed-hosting work packages.
//!
//! ## Why a pool at all
//! Before WP-3e the server held exactly one `rusqlite::Connection` behind one
//! process-wide `std::sync::Mutex`, so **every** request — read or write, any
//! tenant — serialized on it. This module is the read half of the fix: a fixed
//! set of connections that concurrent readers check out and return.
//!
//! ## What it does and does NOT buy you
//! `SQLite` permits exactly **one** writer at a time, even in WAL mode. A pool
//! therefore unlocks *concurrent readers* (and readers running concurrently with
//! the writer, which is what WAL adds) — it does **not** give concurrent writes.
//! That is why [`crate::store::SyncStore`] keeps a single, explicit writer
//! connection and pools only readers: the ceiling is structural rather than
//! hidden behind connections that would otherwise race and collide on
//! `SQLITE_BUSY`. See ADR-031.
//!
//! ## Why hand-rolled
//! `r2d2` + `r2d2_sqlite` is the conventional choice and would work (both are
//! AGPL-compatible), but their value-add — health checks, idle reaping, a
//! background `scheduled-thread-pool` thread — exists to recycle dead *network*
//! connections. A `SQLite` connection is a local file handle that does not die,
//! so the added dependency surface in an AGPL crate buys nothing here. This
//! module is a bounded checkout queue and nothing more.
//!
//! ## Backpressure
//! Checkout is bounded by [`PoolConfig::checkout_timeout`]. A saturated pool
//! makes callers wait, then fails with [`PoolError::Timeout`], which the HTTP
//! layer renders as a retryable `503` rather than blocking forever or
//! surfacing a confusing `500`.
//!
//! **The timeout must always be finite.** Callers reach [`ConnectionPool::get`]
//! from inside `tokio::task::spawn_blocking` (see [`crate::state::AppState`]), so
//! a blocked checkout parks a blocking-pool thread on the [`Condvar`]. That is
//! acceptable — it is what the blocking pool is for — but only because the wait
//! is bounded: saturation then degrades into clean `503`s instead of
//! accumulating parked threads indefinitely. There is no configuration that
//! yields an unbounded wait; `0` means "fail immediately", not "wait forever".
//!
//! ## Lock ordering
//! Two locks exist below the HTTP layer, and they are always taken in this order,
//! never the reverse:
//!
//! 1. a per-tenant slot ([`crate::fairness::TenantLimiter`]), then
//! 2. a database connection — either a pooled reader (here) or the store's
//!    writer mutex.
//!
//! **A caller must never hold one connection while waiting for another.** Every
//! [`crate::store::SyncStore`] method takes exactly one connection guard and
//! passes `&Connection` (or `&Transaction`) down to its helpers, so there is no
//! reader-then-writer or writer-then-reader path. This matters beyond ordinary
//! lock-ordering hygiene: for an **in-memory** store the reader handle *is* the
//! writer mutex, and `std::sync::Mutex` is not reentrant, so a method that took
//! both would self-deadlock on the very configuration the test suites use.
//!
//! [#201]: https://github.com/kafkade/pergamon/issues/201

use std::ops::Deref;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;

/// Default number of pooled reader connections.
///
/// Generous enough that a self-host never notices the bound, small enough that
/// the process keeps a modest number of file handles open.
pub const DEFAULT_READ_POOL_SIZE: usize = 8;

/// Default bound on how long a caller waits for a free connection.
///
/// Matches the store's `PRAGMA busy_timeout`, so a request cannot be stalled
/// materially longer by pool saturation than by `SQLite` contention itself.
pub const DEFAULT_CHECKOUT_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors returned when checking a connection out of the pool.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// No connection became free within [`PoolConfig::checkout_timeout`].
    ///
    /// This is a **transient capacity** condition, not a fault: the caller
    /// should be told to retry (`503`), never given a `500`.
    #[error("timed out waiting {waited_ms}ms for a free database connection")]
    Timeout {
        /// How long the caller actually waited, in milliseconds.
        waited_ms: u64,
    },

    /// The pool's internal lock was poisoned by a panic in another thread.
    #[error("connection pool lock poisoned")]
    Poisoned,
}

/// Sizing and backpressure policy for a [`ConnectionPool`].
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// Number of reader connections to open. Clamped to at least 1.
    pub size: usize,
    /// How long a caller waits for a free connection before giving up.
    pub checkout_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_READ_POOL_SIZE,
            checkout_timeout: DEFAULT_CHECKOUT_TIMEOUT,
        }
    }
}

/// A fixed-size pool of `SQLite` connections.
///
/// Connections are opened eagerly at construction, so a misconfigured database
/// path fails fast at startup rather than on the first request.
#[derive(Debug)]
pub struct ConnectionPool {
    /// Connections not currently checked out.
    idle: Mutex<Vec<Connection>>,
    /// Signalled whenever a connection is returned.
    available: Condvar,
    /// Total number of connections owned by the pool.
    size: usize,
    /// Bound on how long [`Self::get`] waits.
    checkout_timeout: Duration,
}

impl ConnectionPool {
    /// Build a pool of `config.size` connections produced by `open`.
    ///
    /// `open` is called once per connection and is where the caller applies its
    /// per-connection pragmas.
    ///
    /// # Errors
    /// Propagates the first error returned by `open`.
    pub fn new<F>(config: PoolConfig, mut open: F) -> Result<Self, rusqlite::Error>
    where
        F: FnMut() -> Result<Connection, rusqlite::Error>,
    {
        let size = config.size.max(1);
        let mut idle = Vec::with_capacity(size);
        for _ in 0..size {
            idle.push(open()?);
        }
        Ok(Self {
            idle: Mutex::new(idle),
            available: Condvar::new(),
            size,
            checkout_timeout: config.checkout_timeout,
        })
    }

    /// Total number of connections the pool owns.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// How long [`Self::get`] waits before failing with [`PoolError::Timeout`].
    #[must_use]
    pub const fn checkout_timeout(&self) -> Duration {
        self.checkout_timeout
    }

    /// Check a connection out, blocking until one is free or the timeout expires.
    ///
    /// This blocks the calling thread, so callers on an async runtime must run it
    /// inside `tokio::task::spawn_blocking` (see [`crate::state::AppState`]).
    ///
    /// # Errors
    /// Returns [`PoolError::Timeout`] if no connection became free in time, or
    /// [`PoolError::Poisoned`] if another thread panicked holding the pool lock.
    pub fn get(&self) -> Result<PooledConnection<'_>, PoolError> {
        let started = Instant::now();
        let mut idle = self.idle.lock().map_err(|_| PoolError::Poisoned)?;
        loop {
            if let Some(conn) = idle.pop() {
                return Ok(PooledConnection {
                    pool: self,
                    conn: Some(conn),
                });
            }
            // `Condvar` wakeups can be spurious, so re-check against a deadline
            // rather than trusting a single `wait_timeout` return.
            let Some(remaining) = self.checkout_timeout.checked_sub(started.elapsed()) else {
                return Err(PoolError::Timeout {
                    waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                });
            };
            let (guard, timed_out) = self
                .available
                .wait_timeout(idle, remaining)
                .map_err(|_| PoolError::Poisoned)?;
            idle = guard;
            if timed_out.timed_out() && idle.is_empty() {
                return Err(PoolError::Timeout {
                    waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                });
            }
        }
    }

    /// Return a connection to the pool and wake one waiter.
    ///
    /// A poisoned lock means another thread panicked mid-checkout; the
    /// connection is simply dropped, which is safe (the pool then runs with
    /// fewer connections rather than handing out a possibly-corrupt one).
    fn put_back(&self, conn: Connection) {
        if let Ok(mut idle) = self.idle.lock() {
            idle.push(conn);
            drop(idle);
            self.available.notify_one();
        }
    }
}

/// A connection checked out of a [`ConnectionPool`], returned on drop.
///
/// Dereferences to [`Connection`], so pooled code reads exactly like code
/// holding a connection directly.
#[derive(Debug)]
pub struct PooledConnection<'a> {
    /// The pool to return `conn` to.
    pool: &'a ConnectionPool,
    /// Always `Some` until [`Drop`] takes it.
    conn: Option<Connection>,
}

impl Deref for PooledConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        // `conn` is only `None` after `Drop::drop` has taken it, at which point
        // the value can no longer be dereferenced.
        self.conn
            .as_ref()
            .unwrap_or_else(|| unreachable!("pooled connection dereferenced after drop"))
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.put_back(conn);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]
    #![allow(clippy::panic)]

    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    /// A pool of in-memory connections, sized `size`, with a short timeout.
    fn pool(size: usize, timeout_ms: u64) -> ConnectionPool {
        ConnectionPool::new(
            PoolConfig {
                size,
                checkout_timeout: Duration::from_millis(timeout_ms),
            },
            Connection::open_in_memory,
        )
        .unwrap()
    }

    #[test]
    fn size_is_clamped_to_at_least_one() {
        assert_eq!(pool(0, 50).size(), 1);
    }

    #[test]
    fn checkout_returns_a_usable_connection() {
        let pool = pool(2, 500);
        let conn = pool.get().unwrap();
        let n: i64 = conn.query_row("SELECT 1", [], |row| row.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn connections_are_returned_on_drop() {
        let pool = pool(1, 500);
        for _ in 0..5 {
            let conn = pool.get().unwrap();
            drop(conn);
        }
        // Still checkoutable after five round-trips: nothing leaked.
        assert!(pool.get().is_ok());
    }

    /// The whole point of the pool: `size` callers hold connections *at the same
    /// time*. The barrier only trips if all `size` checkouts genuinely overlap,
    /// which the pre-WP-3e single mutexed connection could never do.
    #[test]
    fn checkouts_are_genuinely_concurrent() {
        const N: usize = 6;
        let pool = Arc::new(pool(N, 5_000));
        let barrier = Arc::new(Barrier::new(N));

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let pool = Arc::clone(&pool);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let conn = pool.get().unwrap();
                    // Hold the connection across the rendezvous.
                    barrier.wait();
                    drop(conn);
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn saturated_pool_times_out_instead_of_blocking_forever() {
        let pool = pool(1, 100);
        let held = pool.get().unwrap();
        let err = pool.get().unwrap_err();
        assert!(matches!(err, PoolError::Timeout { .. }), "got {err:?}");
        drop(held);
        // Once the holder releases, checkout succeeds again.
        assert!(pool.get().is_ok());
    }

    /// A waiter blocked on a saturated pool must be woken by a return, not left
    /// to time out.
    #[test]
    fn a_returned_connection_wakes_a_waiter() {
        let pool = Arc::new(pool(1, 10_000));
        let held = pool.get().unwrap();

        let waiter = {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                let started = Instant::now();
                let conn = pool.get().unwrap();
                drop(conn);
                started.elapsed()
            })
        };

        thread::sleep(Duration::from_millis(50));
        drop(held);

        let elapsed = waiter.join().unwrap();
        assert!(
            elapsed < Duration::from_secs(5),
            "waiter should have been woken by the return, waited {elapsed:?}"
        );
    }

    /// A panic while holding a connection must not poison the pool for everyone
    /// else: the guard's `Drop` runs during unwind and returns the connection.
    #[test]
    fn a_panicking_holder_still_returns_its_connection() {
        let pool = Arc::new(pool(1, 500));
        let panicked = {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                let _conn = pool.get().unwrap();
                panic!("boom");
            })
        };
        assert!(panicked.join().is_err());
        assert!(
            pool.get().is_ok(),
            "the connection should have been returned during unwind"
        );
    }
}
