// SPDX-License-Identifier: AGPL-3.0-only

//! The project's single OPAQUE cipher suite (ADR-029, design §1.2).
//!
//! OPAQUE is parameterized by an OPRF group, an AKE key-exchange group + hash,
//! and a key-stretching function (KSF). We pin exactly one suite so every
//! registration and login across all clients and the server is consistent:
//!
//! - **OPRF:** Ristretto255.
//! - **AKE:** Triple-DH over Ristretto255 with SHA-512.
//! - **KSF:** Argon2id (memory-hard), so a stolen verifier still costs an
//!   Argon2id-stretched, OPRF-gated guess per candidate password.
//!
//! This stays inside the ADR-024 crypto toolbox (Ristretto255/X25519 + Argon2id
//! + SHA-512).
//!
//! ## Cross-crate parity (AGPL/Apache boundary)
//! An identical suite is defined client-side in
//! `pergamon-sync`'s `auth` module. The two definitions are intentionally
//! duplicated so no code crosses the AGPL (server) / Apache (client) boundary;
//! OPAQUE wire compatibility depends only on these *parameters*, not on the Rust
//! type identity. The server↔client round-trip integration test is the guardrail
//! against the two drifting apart. **Keep them in sync.**
//!
//! ## `sha2` version note
//! `opaque-ke 4.x` is built on the `digest 0.10` trait generation, so the AKE
//! hash must be `sha2 0.10`'s `Sha512` — imported here via the `sha2-opaque`
//! rename. The crate's `ct_hash` code uses the workspace `sha2 = 0.11`; the two
//! coexist deliberately.

use opaque_ke::{CipherSuite, Ristretto255, TripleDh};

/// The project OPAQUE cipher suite (see module docs).
#[derive(Debug, Clone, Copy)]
pub struct PergamonCipherSuite;

impl CipherSuite for PergamonCipherSuite {
    type OprfCs = Ristretto255;
    type KeyExchange = TripleDh<Ristretto255, sha2_opaque::Sha512>;
    type Ksf = argon2::Argon2<'static>;
}
