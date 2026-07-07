//! Pure, zero-I/O building blocks for the client sync engine (#126).
//!
//! This module carries the **plaintext** semantics that the sync engine
//! encrypts and transports (ADR-022) and the **deterministic conflict
//! resolution** it applies on pull (ADR-023). Everything here is pure
//! computation — no networking, no storage, no clocks read from the OS — so it
//! is exhaustively unit-testable and identical across CLI, iOS, and web
//! (ADR-001 / ADR-007).
//!
//! - [`hlc`] — the hybrid logical clock that provides a total, causally-aware
//!   order with a deterministic `device_id` tiebreak.
//! - [`event`] — the decrypted event body: `entity_type`, `entity_id`, `op`,
//!   `clock`, the observed prior version, changed `fields`, and the blob
//!   manifest. This is exactly what lives inside the ADR-022 ciphertext.
//! - [`merge`] — the table-driven ADR-023 conflict policy: which of the four
//!   strategies (LWW, set-union + observed-remove tombstone, derived-merge,
//!   conflict-copy) applies to which field of which entity, plus the pure merge
//!   functions that resolve a pulled change against local state.
//! - [`review`] — the ADR-023 **derived-merge** for review scheduling: fold a
//!   card's append-only review-log union through the FSRS scheduler to recompute
//!   its schedule losslessly, instead of value-merging counts.

pub mod event;
pub mod hlc;
pub mod merge;
pub mod review;

pub use event::{BlobManifestEntry, ChangeBody, EntityType, Op};
pub use hlc::Hlc;
pub use merge::{
    ConflictStrategy, FieldMerge, MergeDecision, SetMember, SetMergeOutcome, merge_field,
    merge_set_member, strategy_for,
};
pub use review::{DerivedCard, ReviewLogEntry, derive_card_state};
