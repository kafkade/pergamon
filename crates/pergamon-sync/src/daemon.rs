// SPDX-License-Identifier: Apache-2.0

//! The blocking background-sync driver (issue #129).
//!
//! [`run_forever`] ties the pure [`SyncScheduler`](crate::schedule::SyncScheduler)
//! to real time and a real sync round: it runs a round, records success or a
//! retryable/offline failure, computes the next delay, and sleeps — waking early
//! when a caller triggers an out-of-band sync or asks the loop to stop. It is
//! deliberately blocking and runtime-agnostic so a CLI daemon can run it on a
//! dedicated thread and the async web server can run it via
//! `tokio::task::spawn_blocking`.
//!
//! Time and wakeups are injected through the [`Sleeper`] trait so the loop is
//! unit-testable without real clocks; [`control`] returns the production
//! [`ChannelSleeper`] plus a cloneable [`SyncControl`] handle for triggering and
//! shutting the loop down.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::engine::SyncStats;
use crate::error::Result;
use crate::schedule::SyncScheduler;

/// Why a [`Sleeper::wait`] call returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// The requested delay elapsed with no signal.
    Elapsed,
    /// A caller requested an immediate out-of-band sync.
    Triggered,
    /// A caller (or a dropped control handle) asked the loop to stop.
    Shutdown,
}

/// An injectable, interruptible sleep. Blocks for up to `dur`, returning early
/// if a trigger or shutdown arrives.
pub trait Sleeper {
    /// Wait for up to `dur`, reporting why the wait ended.
    fn wait(&self, dur: Duration) -> Wake;
}

/// Internal control signal delivered to a [`ChannelSleeper`].
enum Signal {
    /// Wake now and run a sync round.
    Trigger,
    /// Stop the loop after the current wait.
    Shutdown,
}

/// A cloneable handle for driving a running [`run_forever`] loop.
///
/// Dropping every clone disconnects the channel, which the loop treats as a
/// shutdown request so orphaned workers do not spin forever.
#[derive(Clone)]
pub struct SyncControl {
    tx: Sender<Signal>,
}

impl SyncControl {
    /// Ask the loop to run a sync round immediately, cutting its current sleep
    /// short. Returns `false` if the loop has already stopped.
    #[must_use]
    pub fn trigger(&self) -> bool {
        self.tx.send(Signal::Trigger).is_ok()
    }

    /// Ask the loop to stop after its current round/wait. Returns `false` if the
    /// loop has already stopped.
    #[must_use]
    pub fn shutdown(&self) -> bool {
        self.tx.send(Signal::Shutdown).is_ok()
    }
}

/// The production [`Sleeper`]: blocks on an mpsc channel with a timeout.
pub struct ChannelSleeper {
    rx: Receiver<Signal>,
}

impl Sleeper for ChannelSleeper {
    fn wait(&self, dur: Duration) -> Wake {
        match self.rx.recv_timeout(dur) {
            Ok(Signal::Trigger) => Wake::Triggered,
            Ok(Signal::Shutdown) | Err(RecvTimeoutError::Disconnected) => Wake::Shutdown,
            Err(RecvTimeoutError::Timeout) => Wake::Elapsed,
        }
    }
}

/// Create a [`SyncControl`]/[`ChannelSleeper`] pair for [`run_forever`].
#[must_use]
pub fn control() -> (SyncControl, ChannelSleeper) {
    let (tx, rx) = mpsc::channel();
    (SyncControl { tx }, ChannelSleeper { rx })
}

/// A tiny deterministic PRNG (splitmix64) yielding jitter fractions.
///
/// Produces values in `[0.0, 1.0)`, seeded from entropy in production and from a
/// fixed seed in tests; the sync loop never needs cryptographic randomness for
/// jitter.
pub struct Jitter {
    state: u64,
}

impl Jitter {
    /// A fixed golden-ratio constant used for seeding and mixing.
    const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Seed deterministically (tests, reproducibility).
    #[must_use]
    pub const fn from_seed(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Seed from the wall clock (production).
    #[must_use]
    pub fn from_entropy() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(Self::GOLDEN, |d| {
                u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(Self::GOLDEN)
            });
        Self::from_seed(nanos ^ Self::GOLDEN)
    }

    /// The next jitter fraction in `[0.0, 1.0)`.
    #[allow(clippy::cast_precision_loss)] // 53-bit mantissa slice is exact in f64
    pub fn next01(&mut self) -> f64 {
        self.state = self.state.wrapping_add(Self::GOLDEN);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

impl Default for Jitter {
    fn default() -> Self {
        Self::from_entropy()
    }
}

/// What a single background round did.
#[derive(Debug, Clone)]
pub enum RoundOutcome {
    /// A round completed; carries what it pushed/applied.
    Synced(SyncStats),
    /// A retryable/offline failure; the loop backed off and will retry.
    Offline(String),
}

/// An observation emitted after each round, for logging/metrics.
#[derive(Debug, Clone)]
pub struct RoundReport {
    /// The round's result.
    pub outcome: RoundOutcome,
    /// Consecutive retryable failures after this round (0 when healthy).
    pub consecutive_failures: u32,
    /// The delay the loop will wait before the next round.
    pub next_delay: Duration,
}

/// Run background sync until asked to stop.
///
/// Each iteration runs `round` once, updates `scheduler` (success clears
/// backoff; a retryable error deepens it), reports via `observe`, then sleeps
/// for the scheduled delay via `sleeper` — waking early on a trigger. A
/// **fatal** (non-retryable) error from `round` stops the loop and is returned;
/// a [`Wake::Shutdown`] stops it cleanly with `Ok(())`.
///
/// # Errors
/// Returns the first non-retryable [`SyncError`](crate::error::SyncError) a
/// round produces.
pub fn run_forever<Round, Obs, S>(
    mut round: Round,
    mut scheduler: SyncScheduler,
    sleeper: &S,
    mut jitter: Jitter,
    mut observe: Obs,
) -> Result<()>
where
    Round: FnMut() -> Result<SyncStats>,
    Obs: FnMut(&RoundReport),
    S: Sleeper + ?Sized,
{
    loop {
        let outcome = match round() {
            Ok(stats) => {
                scheduler.record_success();
                RoundOutcome::Synced(stats)
            }
            Err(e) if e.is_retryable() => {
                scheduler.record_failure();
                RoundOutcome::Offline(e.to_string())
            }
            Err(e) => return Err(e),
        };
        let next_delay = scheduler.next_delay(jitter.next01());
        observe(&RoundReport {
            outcome,
            consecutive_failures: scheduler.consecutive_failures(),
            next_delay,
        });
        match sleeper.wait(next_delay) {
            Wake::Shutdown => return Ok(()),
            Wake::Elapsed | Wake::Triggered => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::duration_suboptimal_units)]

    use std::cell::RefCell;

    use super::*;
    use crate::error::SyncError;
    use crate::schedule::{BackoffPolicy, SyncScheduler};

    /// A scripted sleeper: returns a preset [`Wake`] per call and records the
    /// delays it was asked to wait.
    struct ScriptedSleeper {
        wakes: RefCell<std::vec::IntoIter<Wake>>,
        delays: RefCell<Vec<Duration>>,
    }

    impl ScriptedSleeper {
        fn new(wakes: Vec<Wake>) -> Self {
            Self {
                wakes: RefCell::new(wakes.into_iter()),
                delays: RefCell::new(Vec::new()),
            }
        }
    }

    impl Sleeper for ScriptedSleeper {
        fn wait(&self, dur: Duration) -> Wake {
            self.delays.borrow_mut().push(dur);
            self.wakes.borrow_mut().next().unwrap_or(Wake::Shutdown)
        }
    }

    fn scheduler() -> SyncScheduler {
        SyncScheduler::new(
            Duration::from_secs(100),
            BackoffPolicy::new(Duration::from_secs(2), Duration::from_secs(60), 2.0),
        )
    }

    #[test]
    fn stops_cleanly_on_shutdown() {
        let sleeper = ScriptedSleeper::new(vec![Wake::Shutdown]);
        let mut rounds = 0;
        let res = run_forever(
            || {
                rounds += 1;
                Ok(SyncStats::default())
            },
            scheduler(),
            &sleeper,
            Jitter::from_seed(1),
            |_| {},
        );
        assert!(res.is_ok());
        assert_eq!(rounds, 1);
        assert_eq!(sleeper.delays.borrow()[0], Duration::from_secs(100));
    }

    #[test]
    fn backs_off_on_offline_then_recovers_interval() {
        // round 1: offline → backoff (retry 0 → 2s). round 2: success → interval.
        let sleeper = ScriptedSleeper::new(vec![Wake::Elapsed, Wake::Shutdown]);
        let mut call = 0;
        let reports = RefCell::new(Vec::new());
        let res = run_forever(
            || {
                call += 1;
                if call == 1 {
                    Err(SyncError::Transport("offline".into()))
                } else {
                    Ok(SyncStats {
                        pushed: 1,
                        applied: 0,
                    })
                }
            },
            scheduler(),
            &sleeper,
            Jitter::from_seed(42),
            |r| reports.borrow_mut().push(r.clone()),
        );
        assert!(res.is_ok());
        let delays = sleeper.delays.borrow();
        // First (offline) delay is within the backoff band [1s, 2s].
        assert!(delays[0] >= Duration::from_secs(1) && delays[0] <= Duration::from_secs(2));
        // After recovery, the steady-state interval.
        assert_eq!(delays[1], Duration::from_secs(100));
        assert_eq!(reports.borrow().len(), 2);
    }

    #[test]
    fn fatal_error_stops_and_propagates() {
        let sleeper = ScriptedSleeper::new(vec![Wake::Elapsed]);
        let res = run_forever(
            || Err(SyncError::Protocol("bad frame".into())),
            scheduler(),
            &sleeper,
            Jitter::from_seed(1),
            |_| {},
        );
        assert!(matches!(res, Err(SyncError::Protocol(_))));
        // Never slept: the fatal error returned before scheduling.
        assert!(sleeper.delays.borrow().is_empty());
    }

    #[test]
    fn trigger_wakes_and_continues() {
        let sleeper = ScriptedSleeper::new(vec![Wake::Triggered, Wake::Shutdown]);
        let mut rounds = 0;
        let res = run_forever(
            || {
                rounds += 1;
                Ok(SyncStats::default())
            },
            scheduler(),
            &sleeper,
            Jitter::from_seed(1),
            |_| {},
        );
        assert!(res.is_ok());
        assert_eq!(rounds, 2);
    }

    #[test]
    fn jitter_is_in_unit_interval() {
        let mut j = Jitter::from_seed(12345);
        for _ in 0..10_000 {
            let x = j.next01();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn control_trigger_and_shutdown_map_to_wakes() {
        let (ctl, sleeper) = control();
        assert!(ctl.trigger());
        assert_eq!(sleeper.wait(Duration::from_secs(10)), Wake::Triggered);
        assert!(ctl.shutdown());
        assert_eq!(sleeper.wait(Duration::from_secs(10)), Wake::Shutdown);
    }

    #[test]
    fn dropped_control_disconnects_to_shutdown() {
        let (ctl, sleeper) = control();
        drop(ctl);
        assert_eq!(sleeper.wait(Duration::from_millis(1)), Wake::Shutdown);
    }
}
