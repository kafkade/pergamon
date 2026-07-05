// SPDX-License-Identifier: Apache-2.0

//! # pergamon-crypto
//!
//! Client-side **end-to-end encryption** for pergamon's optional multi-device
//! sync, implementing the scheme fixed by ADR-024 on top of the ADR-022 wire
//! envelope.
//!
//! This crate is **Apache-2.0** (like every other client/core crate). The
//! AGPL-3.0 `pergamon-sync-server` never links it: the server only stores and
//! relays the opaque ciphertext this crate produces. Keeping all cryptography
//! here — and out of `pergamon-core` — preserves ADR-001's zero-I/O core, since
//! key generation and nonces require a CSPRNG.
//!
//! ## What lives here
//!
//! - [`primitives`] — thin, typed wrappers over the audited primitives ADR-024
//!   chose: XChaCha20-Poly1305, HKDF-SHA-256, HMAC-SHA-256, X25519 sealed box,
//!   Ed25519, Argon2id, BLAKE3.
//! - [`hierarchy`] — the key schedule: Account Root Key (ARK), `account_id`,
//!   `account_stream_key`, per-epoch account content keys (`ACK_e`), per-event
//!   keys, and convergent blob keys.
//! - [`envelope`] — authenticated encryption of ADR-022 event bodies, binding
//!   the server-visible header as AEAD associated data, plus `entity_ref`
//!   blinding.
//! - [`blob`] — convergent (content-derived) encryption of immutable blobs so
//!   ADR-022's ciphertext-hash dedup keeps working under E2EE.
//! - [`device`] — per-device X25519 + Ed25519 keypairs and signed device
//!   records.
//! - [`enrollment`] — device-to-device onboarding: Short Authentication String
//!   (SAS) verification and sealed enrollment bundles.
//! - [`attestation`] — Ed25519 trust and revocation attestations.
//! - [`recovery`] — the optional, opt-in Argon2id-wrapped recovery blob.
//! - [`rotation`] — key-epoch rotation on device revocation.
//!
//! Every derivation is deterministic and unit-tested with known-answer vectors;
//! everything that needs randomness (key/nonce generation) takes it from the OS
//! CSPRNG.

#![forbid(unsafe_code)]

/// Version string of the crate, matching the crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod attestation;
pub mod blob;
pub mod device;
pub mod enrollment;
pub mod envelope;
pub mod error;
pub mod hierarchy;
pub mod primitives;
pub mod recovery;
pub mod rotation;

pub use attestation::{
    Attestation, AttestationKind, SignedAttestation, attest_revoke, attest_trust,
};
pub use blob::{EncryptedBlob, decrypt_blob, encrypt_blob};
pub use device::{DeviceKeypairs, DeviceRecord, SignedDeviceRecord};
pub use enrollment::{
    EnrollmentBundle, EnrollmentPeer, Sas, open_enrollment_bundle, seal_enrollment_bundle,
};
pub use envelope::{EventHeader, decrypt_event, encrypt_event, entity_ref};
pub use error::{CryptoError, Result};
pub use hierarchy::{AccountContentKey, AccountId, AccountRootKey};
pub use recovery::{RecoveryBlob, enable_recovery, generate_recovery_code, recover};
pub use rotation::{RewrapRecipient, RewrappedKey, open_rewrapped, rotate_and_rewrap};
