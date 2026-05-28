//! FSRS-5 spaced repetition scheduler — pure computation, no I/O.
//!
//! Implements the Free Spaced Repetition Scheduler (FSRS) v5 algorithm
//! as specified in ADR-005. This module contains only deterministic
//! arithmetic over review state and parameters — no networking, no
//! file system access, no database coupling.
//!
//! Reference: <https://github.com/open-spaced-repetition/fsrs-rs>

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

// ======================================================================
// Constants
// ======================================================================

/// Minimum stability value (days).
const S_MIN: f64 = 0.001;

/// Maximum stability value (days).
const S_MAX: f64 = 36500.0;

/// Minimum difficulty value.
const D_MIN: f64 = 1.0;

/// Maximum difficulty value.
const D_MAX: f64 = 10.0;

/// FSRS-5 fixed decay parameter (negative by convention).
const DECAY: f64 = -0.5;

/// Precomputed factor for the power forgetting curve.
/// `factor = 0.9^(1/DECAY) - 1 = 0.9^(-2) - 1 ≈ 0.2346`
const FACTOR: f64 = 0.234_567_901_234_568; // 19/81

/// Default FSRS-5 weights (19 parameters).
///
/// Sourced from the reference implementation's default parameter set.
pub const DEFAULT_WEIGHTS: [f64; 19] = [
    0.212,  // w0:  initial stability for Again
    1.2931, // w1:  initial stability for Hard
    2.3065, // w2:  initial stability for Good
    8.2956, // w3:  initial stability for Easy
    6.4133, // w4:  initial difficulty base
    0.8334, // w5:  initial difficulty scaling
    3.0194, // w6:  difficulty update rate
    0.001,  // w7:  mean reversion weight
    1.8722, // w8:  success stability: exp factor
    0.1666, // w9:  success stability: stability exponent
    0.796,  // w10: success stability: retrievability scaling
    1.4835, // w11: failure stability: base
    0.0614, // w12: failure stability: difficulty exponent
    0.2629, // w13: failure stability: stability exponent
    1.6483, // w14: failure stability: retrievability scaling
    0.6014, // w15: hard penalty
    1.8729, // w16: easy bonus
    0.5425, // w17: short-term stability: exp factor
    0.0912, // w18: short-term stability: offset
];

// ======================================================================
// Enums
// ======================================================================

/// State of a review card in the FSRS lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    /// Card has never been reviewed.
    New,
    /// Card is in the initial learning phase.
    Learning,
    /// Card is in the long-term review phase.
    Review,
    /// Card was forgotten and is being relearned.
    Relearning,
}

impl CardState {
    /// Returns the canonical string representation used in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Learning => "learning",
            Self::Review => "review",
            Self::Relearning => "relearning",
        }
    }
}

impl fmt::Display for CardState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CardState {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "new" => Ok(Self::New),
            "learning" => Ok(Self::Learning),
            "review" => Ok(Self::Review),
            "relearning" => Ok(Self::Relearning),
            other => Err(CoreError::UnknownCardState(other.to_owned())),
        }
    }
}

/// User rating after reviewing a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    /// Forgot the material — schedule again soon.
    Again = 1,
    /// Recalled with significant difficulty.
    Hard = 2,
    /// Recalled correctly.
    Good = 3,
    /// Recalled effortlessly.
    Easy = 4,
}

impl Rating {
    /// Parse a rating from its integer value (1–4).
    #[must_use]
    pub const fn from_value(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Again),
            2 => Some(Self::Hard),
            3 => Some(Self::Good),
            4 => Some(Self::Easy),
            _ => None,
        }
    }

    /// Return the integer value of this rating.
    #[must_use]
    pub const fn value(self) -> u32 {
        self as u32
    }
}

impl fmt::Display for Rating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Again => f.write_str("Again"),
            Self::Hard => f.write_str("Hard"),
            Self::Good => f.write_str("Good"),
            Self::Easy => f.write_str("Easy"),
        }
    }
}

// ======================================================================
// Memory state
// ======================================================================

/// FSRS memory state: stability and difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemoryState {
    /// How many days until recall probability drops to 90%.
    pub stability: f64,
    /// How hard the material is (1.0 = easiest, 10.0 = hardest).
    pub difficulty: f64,
}

// ======================================================================
// Parameters
// ======================================================================

/// FSRS scheduling parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameters {
    /// 19 FSRS-5 model weights.
    pub weights: [f64; 19],
    /// Target recall probability (0.0–1.0, default 0.9).
    pub desired_retention: f64,
    /// Maximum review interval in days (default 36500 ≈ 100 years).
    pub maximum_interval: f64,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            weights: DEFAULT_WEIGHTS,
            desired_retention: 0.9,
            maximum_interval: 36500.0,
        }
    }
}

// ======================================================================
// Scheduler
// ======================================================================

/// FSRS-5 spaced repetition scheduler.
///
/// All methods are pure functions operating on the provided state and
/// parameters. The scheduler has no side effects and no I/O.
pub struct Scheduler {
    w: [f64; 19],
    desired_retention: f64,
    maximum_interval: f64,
}

impl Scheduler {
    /// Create a new scheduler with the given parameters.
    #[must_use]
    pub const fn new(params: &Parameters) -> Self {
        Self {
            w: params.weights,
            desired_retention: params.desired_retention,
            maximum_interval: params.maximum_interval,
        }
    }

    /// Create a scheduler with default FSRS-5 parameters.
    #[must_use]
    pub fn default_v5() -> Self {
        Self::new(&Parameters::default())
    }

    // ------------------------------------------------------------------
    // Core FSRS-5 formulas
    // ------------------------------------------------------------------

    /// Power forgetting curve: probability of recall after `elapsed_days`.
    ///
    /// `R(t) = (factor * t / S + 1)^DECAY`
    #[must_use]
    pub fn retrievability(&self, stability: f64, elapsed_days: f64) -> f64 {
        let s = stability.clamp(S_MIN, S_MAX);
        (FACTOR * elapsed_days / s + 1.0).powf(DECAY)
    }

    /// Compute the interval (in days) for a given stability and desired retention.
    #[must_use]
    fn next_interval(&self, stability: f64) -> f64 {
        let s = stability.clamp(S_MIN, S_MAX);
        // 1/DECAY = 1/(-0.5) = -2
        let interval = s / FACTOR * (self.desired_retention.powi(-2) - 1.0);
        interval.clamp(1.0, self.maximum_interval)
    }

    /// Initial stability based on the first rating.
    #[must_use]
    const fn init_stability(&self, rating: Rating) -> f64 {
        let idx = rating.value() as usize - 1; // 0..3
        self.w[idx].max(S_MIN)
    }

    /// Initial difficulty based on the first rating.
    #[must_use]
    fn init_difficulty(&self, rating: Rating) -> f64 {
        let r = f64::from(rating.value());
        let d = self.w[4] - (self.w[5] * (r - 1.0)).exp() + 1.0;
        d.clamp(D_MIN, D_MAX)
    }

    /// Next difficulty after a review.
    #[must_use]
    fn next_difficulty(&self, d: f64, rating: Rating) -> f64 {
        let r = f64::from(rating.value());
        let delta_d = -self.w[6] * (r - 3.0);
        // Linear damping: scale delta by (10 - D) / 9
        let damped = delta_d * (10.0 - d) / 9.0;
        let new_d = d + damped;
        // Mean reversion toward initial difficulty for Easy
        let init_d = self.init_difficulty(Rating::Easy);
        let reverted = self.w[7].mul_add(init_d - new_d, new_d);
        reverted.clamp(D_MIN, D_MAX)
    }

    /// Stability after a successful recall (rating ∈ {Hard, Good, Easy}).
    #[must_use]
    fn stability_after_success(&self, s: f64, d: f64, r: f64, rating: Rating) -> f64 {
        let hard_penalty = if rating == Rating::Hard {
            self.w[15]
        } else {
            1.0
        };
        let easy_bonus = if rating == Rating::Easy {
            self.w[16]
        } else {
            1.0
        };

        let inner = self.w[8].exp()
            * (11.0 - d)
            * s.powf(-self.w[9])
            * ((1.0 - r) * self.w[10]).exp_m1()
            * hard_penalty;
        let new_s = s * inner.mul_add(easy_bonus, 1.0);

        new_s.clamp(S_MIN, S_MAX)
    }

    /// Stability after a lapse (rating = Again).
    #[must_use]
    fn stability_after_failure(&self, s: f64, d: f64, r: f64) -> f64 {
        let new_s = self.w[11]
            * d.powf(-self.w[12])
            * ((s + 1.0).powf(self.w[13]) - 1.0)
            * ((1.0 - r) * self.w[14]).exp();

        new_s.clamp(S_MIN, S_MAX)
    }

    /// Short-term stability adjustment (same-day reviews, `elapsed_days` = 0).
    ///
    /// In FSRS-5, the `S^(-w[19])` term has w[19]=0, so `S^0 = 1`.
    #[must_use]
    fn stability_short_term(&self, s: f64, rating: Rating) -> f64 {
        let r = f64::from(rating.value());
        let sinc = (self.w[17] * (r - 3.0 + self.w[18])).exp();
        // For rating >= 2 (Hard/Good/Easy), clamp sinc to >= 1.0
        let sinc = if rating.value() >= 2 {
            sinc.max(1.0)
        } else {
            sinc
        };
        (s * sinc).clamp(S_MIN, S_MAX)
    }

    // ------------------------------------------------------------------
    // Public scheduling API
    // ------------------------------------------------------------------

    /// Schedule the next review for a card given the user's rating.
    ///
    /// Returns the updated memory state, the next interval in days,
    /// and the new card state.
    #[must_use]
    pub fn schedule(
        &self,
        _current_state: CardState,
        memory: Option<MemoryState>,
        elapsed_days: f64,
        rating: Rating,
    ) -> ScheduleOutput {
        memory.map_or_else(
            || {
                // New card or card without memory state — initialize from scratch
                let s = self.init_stability(rating);
                let d = self.init_difficulty(rating);
                let interval = self.next_interval(s);
                let next_state = if rating == Rating::Again {
                    CardState::Learning
                } else {
                    CardState::Review
                };
                ScheduleOutput {
                    memory: MemoryState {
                        stability: s,
                        difficulty: d,
                    },
                    scheduled_days: interval,
                    next_state,
                }
            },
            |mem| {
                let s = mem.stability.clamp(S_MIN, S_MAX);
                let d = mem.difficulty.clamp(D_MIN, D_MAX);

                // Compute new stability
                let new_s = if elapsed_days < 0.5 {
                    // Same-day review: use short-term adjustment
                    self.stability_short_term(s, rating)
                } else {
                    let r = self.retrievability(s, elapsed_days);
                    if rating == Rating::Again {
                        self.stability_after_failure(s, d, r)
                    } else {
                        self.stability_after_success(s, d, r, rating)
                    }
                };

                let new_d = self.next_difficulty(d, rating);
                let interval = self.next_interval(new_s);

                let next_state = if rating == Rating::Again {
                    CardState::Relearning
                } else {
                    CardState::Review
                };

                ScheduleOutput {
                    memory: MemoryState {
                        stability: new_s,
                        difficulty: new_d,
                    },
                    scheduled_days: interval,
                    next_state,
                }
            },
        )
    }
}

/// Output of a scheduling decision.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleOutput {
    /// Updated memory state after the review.
    pub memory: MemoryState,
    /// Number of days until the next review.
    pub scheduled_days: f64,
    /// New card state after the review.
    pub next_state: CardState,
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler() -> Scheduler {
        Scheduler::default_v5()
    }

    #[test]
    fn default_parameters_are_valid() {
        let params = Parameters::default();
        assert_eq!(params.weights.len(), 19);
        assert!((params.desired_retention - 0.9).abs() < f64::EPSILON);
        assert!((params.maximum_interval - 36500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn retrievability_at_zero_is_one() {
        let s = scheduler();
        let r = s.retrievability(1.0, 0.0);
        assert!((r - 1.0).abs() < 1e-10, "R(0) should be 1.0, got {r}");
    }

    #[test]
    fn retrievability_decreases_over_time() {
        let s = scheduler();
        let r1 = s.retrievability(5.0, 1.0);
        let r5 = s.retrievability(5.0, 5.0);
        let r10 = s.retrievability(5.0, 10.0);
        assert!(r1 > r5, "R(1) should > R(5)");
        assert!(r5 > r10, "R(5) should > R(10)");
        assert!(r1 < 1.0);
        assert!(r10 > 0.0);
    }

    #[test]
    fn retrievability_at_stability_is_about_90_percent() {
        let s = scheduler();
        // When elapsed_days == stability, R should be ≈ 0.9
        let r = s.retrievability(10.0, 10.0);
        assert!((r - 0.9).abs() < 0.01, "R(S) should be ≈ 0.9, got {r}");
    }

    #[test]
    fn next_interval_positive_for_default_retention() {
        let s = scheduler();
        let interval = s.next_interval(5.0);
        assert!(
            interval > 0.0,
            "interval should be positive, got {interval}"
        );
        // For desired_retention=0.9, interval ≈ stability
        assert!(
            (interval - 5.0).abs() < 0.5,
            "interval should be ≈ 5.0, got {interval}"
        );
    }

    #[test]
    fn next_interval_clamped_to_max() {
        let s = scheduler();
        let interval = s.next_interval(50000.0);
        assert!(
            interval <= 36500.0 + 1.0,
            "interval should be clamped to max, got {interval}"
        );
        assert!(
            interval >= 36499.0,
            "interval should be near max, got {interval}"
        );
    }

    #[test]
    fn init_stability_per_rating() {
        let s = scheduler();
        let s1 = s.init_stability(Rating::Again);
        let s2 = s.init_stability(Rating::Hard);
        let s3 = s.init_stability(Rating::Good);
        let s4 = s.init_stability(Rating::Easy);
        assert!(s1 < s2);
        assert!(s2 < s3);
        assert!(s3 < s4);
    }

    #[test]
    fn init_difficulty_decreases_with_easier_rating() {
        let s = scheduler();
        let d1 = s.init_difficulty(Rating::Again);
        let d4 = s.init_difficulty(Rating::Easy);
        assert!(d1 > d4, "Again should be harder than Easy: {d1} vs {d4}");
    }

    #[test]
    fn schedule_new_card_good() {
        let s = scheduler();
        let out = s.schedule(CardState::New, None, 0.0, Rating::Good);
        assert_eq!(out.next_state, CardState::Review);
        assert!(out.memory.stability > 0.0);
        assert!(out.memory.difficulty >= D_MIN);
        assert!(out.memory.difficulty <= D_MAX);
        assert!(out.scheduled_days >= 1.0);
    }

    #[test]
    fn schedule_new_card_again_goes_to_learning() {
        let s = scheduler();
        let out = s.schedule(CardState::New, None, 0.0, Rating::Again);
        assert_eq!(out.next_state, CardState::Learning);
    }

    #[test]
    fn schedule_review_again_goes_to_relearning() {
        let s = scheduler();
        let first = s.schedule(CardState::New, None, 0.0, Rating::Good);
        let second = s.schedule(
            CardState::Review,
            Some(first.memory),
            first.scheduled_days,
            Rating::Again,
        );
        assert_eq!(second.next_state, CardState::Relearning);
        assert!(
            second.memory.stability < first.memory.stability,
            "lapse should reduce stability"
        );
    }

    #[test]
    fn schedule_review_good_increases_stability() {
        let s = scheduler();
        let first = s.schedule(CardState::New, None, 0.0, Rating::Good);
        let second = s.schedule(
            CardState::Review,
            Some(first.memory),
            first.scheduled_days,
            Rating::Good,
        );
        assert!(
            second.memory.stability > first.memory.stability,
            "Good review should increase stability: {} vs {}",
            second.memory.stability,
            first.memory.stability,
        );
    }

    #[test]
    fn schedule_easy_longer_than_good() {
        let s = scheduler();
        let first = s.schedule(CardState::New, None, 0.0, Rating::Good);
        let good = s.schedule(
            CardState::Review,
            Some(first.memory),
            first.scheduled_days,
            Rating::Good,
        );
        let easy = s.schedule(
            CardState::Review,
            Some(first.memory),
            first.scheduled_days,
            Rating::Easy,
        );
        assert!(
            easy.scheduled_days > good.scheduled_days,
            "Easy should schedule further than Good: {} vs {}",
            easy.scheduled_days,
            good.scheduled_days,
        );
    }

    #[test]
    fn difficulty_increases_on_again() {
        let s = scheduler();
        let first = s.schedule(CardState::New, None, 0.0, Rating::Good);
        let after_again = s.schedule(
            CardState::Review,
            Some(first.memory),
            first.scheduled_days,
            Rating::Again,
        );
        assert!(
            after_again.memory.difficulty > first.memory.difficulty,
            "Again should increase difficulty"
        );
    }

    #[test]
    fn difficulty_stays_in_bounds() {
        let s = scheduler();
        // Many Again reviews should not push difficulty beyond D_MAX
        let mut mem = s.schedule(CardState::New, None, 0.0, Rating::Again).memory;
        for _ in 0..100 {
            let out = s.schedule(CardState::Relearning, Some(mem), 1.0, Rating::Again);
            mem = out.memory;
        }
        assert!(
            mem.difficulty <= D_MAX,
            "difficulty should be <= {D_MAX}, got {}",
            mem.difficulty
        );
        assert!(
            mem.difficulty >= D_MIN,
            "difficulty should be >= {D_MIN}, got {}",
            mem.difficulty
        );
    }

    #[test]
    fn schedule_is_deterministic() {
        let s = scheduler();
        let a = s.schedule(CardState::New, None, 0.0, Rating::Good);
        let b = s.schedule(CardState::New, None, 0.0, Rating::Good);
        assert_eq!(a, b, "same inputs should produce same outputs");
    }

    #[test]
    fn card_state_round_trip() {
        let states = [
            CardState::New,
            CardState::Learning,
            CardState::Review,
            CardState::Relearning,
        ];
        for state in states {
            let s = state.to_string();
            let parsed: CardState = s.parse().unwrap_or_else(|e| {
                let _ = e;
                unreachable!("failed to parse CardState from {s:?}")
            });
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn rating_from_value() {
        assert_eq!(Rating::from_value(1), Some(Rating::Again));
        assert_eq!(Rating::from_value(2), Some(Rating::Hard));
        assert_eq!(Rating::from_value(3), Some(Rating::Good));
        assert_eq!(Rating::from_value(4), Some(Rating::Easy));
        assert_eq!(Rating::from_value(0), None);
        assert_eq!(Rating::from_value(5), None);
    }
}
