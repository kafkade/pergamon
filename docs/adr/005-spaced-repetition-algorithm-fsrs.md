# ADR-005: Spaced Repetition Algorithm — FSRS

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

One of pergamon’s key differentiators is replacing the highlight review and recall workflow commonly associated with Readwise. This means pergamon must not only store highlights but also schedule review sessions intelligently. The scheduling algorithm matters because it directly shapes user experience: too many reviews create fatigue, too few reduce retention, and poorly tuned intervals make the system feel wasteful.

The historical default in many spaced repetition systems is SM-2. It is simple and well known, but it is no longer the state of the art. Modern research and open-source practice have moved toward FSRS, which better models memory stability and difficulty and can be optimized from user review history. For pergamon, that matters because the application is intended to manage a wide variety of content and highlights over long periods. A more adaptive algorithm is a better fit than a legacy heuristic.

Architecturally, scheduling logic belongs in pergamon-core. It is pure computation over review history and current state and does not require networking, filesystem access, or database access. The CLI and TUI should present queues and collect review responses, but the scheduler itself should remain platform-neutral and testable.

Because pergamon is local-first and solo-developed, the chosen algorithm must also be understandable, open, and implementable without relying on opaque remote services.

## Decision

pergamon will adopt FSRS (Free Spaced Repetition Scheduler) as its spaced repetition algorithm instead of SM-2.

FSRS logic will live in `pergamon-core` as pure computation. Each highlight under review tracking will store FSRS-related state including:

- `stability`
- `difficulty`
- `elapsed_days`
- `scheduled_days`
- `reps`
- `lapses`
- `state` (`new`, `learning`, `review`, `relearning`)

Review actions presented to the user will be:

- Again
- Hard
- Good
- Easy

`pergamon-core` will compute due status and daily review queues based on stored review state and current date input supplied by the caller. `pergamon-cli` and the TUI will handle queue presentation, keyboard interaction, and persistence of review results. Parameter optimization is permitted in the architecture, but initial implementation may ship with sensible default parameters before user-specific tuning is introduced.

## Consequences

### Positive

- Provides a more modern scheduling model than SM-2.
- Can achieve better retention with fewer reviews over time.
- Fits cleanly into a pure, testable core computation layer.
- Leaves room for future parameter optimization from local review history.
- Gives pergamon a serious learning workflow rather than a superficial reminder system.

### Negative

- More complex to understand and implement than SM-2.
- Requires more state per highlight than simpler schedulers.
- Parameter tuning and migration strategy may add later complexity.
- Users familiar with traditional Anki-like interval intuition may need adjustment.

## Rejected Alternatives

- **SM-2**: rejected because it is older, less flexible, and generally inferior to FSRS for long-term scheduling quality.
- **A custom heuristic scheduler**: rejected because it would be harder to validate and would reinvent work already done by the FSRS community.
- **No spaced repetition, only resurfacing recent highlights**: rejected because pergamon explicitly aims to replace Readwise-style review, not just archive highlights.
- **Implement FSRS outside pergamon-core in the CLI**: rejected because scheduling is domain logic and should remain reusable, deterministic, and platform-neutral.
