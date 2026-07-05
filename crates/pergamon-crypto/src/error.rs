// SPDX-License-Identifier: Apache-2.0

//! Error types for `pergamon-crypto`.
//!
//! All fallible operations return [`Result`]. Errors are deliberately coarse
//! and free of secret-dependent detail so they never become an oracle: an
//! attacker learns only *that* an operation failed (e.g. authentication), never
//! *why* at a level that would distinguish key material.

/// Convenience alias for results returned by this crate.
pub type Result<T> = core::result::Result<T, CryptoError>;

/// Errors produced by the client-side cryptography.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CryptoError {
    /// AEAD authentication or decryption failed (wrong key, wrong AAD, or a
    /// tampered ciphertext). Intentionally undifferentiated.
    #[error("decryption failed: ciphertext could not be authenticated")]
    Decryption,

    /// A ciphertext, key, nonce, or signature had an invalid length or format.
    #[error("malformed cryptographic input: {0}")]
    Malformed(&'static str),

    /// An Ed25519 signature did not verify against the expected key.
    #[error("signature verification failed")]
    BadSignature,

    /// Argon2id password stretching failed (invalid parameters).
    #[error("key-derivation (argon2id) failed")]
    KeyDerivation,

    /// The OS CSPRNG failed to produce randomness.
    #[error("secure random number generation failed")]
    Random,
}
