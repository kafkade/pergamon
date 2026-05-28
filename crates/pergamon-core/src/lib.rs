//! # pergamon-core
//!
//! Pure-computation core for the pergamon information system. This crate
//! contains the domain model, content type taxonomy, document state machine,
//! and spaced repetition engine (FSRS).
//!
//! It has **zero I/O dependencies** — no networking, no file system access,
//! no platform APIs. All I/O happens in platform-specific code (CLI, iOS,
//! web). This keeps the core testable (pure functions) and compilable to WASM.
//!
//! See `docs/adr/001-zero-io-core-library.md` for the rationale and
//! invariants that must be preserved.

/// Version string of the pergamon-core library, matching the crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod content_type;
pub mod error;
pub mod fsrs;
pub mod model;
pub mod status;

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
