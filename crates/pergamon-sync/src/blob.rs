// SPDX-License-Identifier: Apache-2.0

//! The client blob-plaintext store used for blob sync.
//!
//! Large immutable payloads (raw HTML snapshots, PDFs, extracted text) are not
//! carried inline in an event body; the body carries a *blob manifest* of
//! ciphertext hashes and the convergent-key inputs. On push the engine needs the
//! plaintext to encrypt and upload; on apply it stores the fetched plaintext.
//! This trait abstracts that local plaintext store, keyed by the blake3
//! plaintext hash (lowercase hex) recorded in the manifest.
#![allow(clippy::significant_drop_tightening)]

use std::collections::HashMap;
use std::sync::Mutex;

/// A local store of blob *plaintext*, addressed by lowercase-hex blake3 hash.
pub trait BlobStore {
    /// Load a blob's plaintext by its plaintext hash, if present locally.
    ///
    /// # Errors
    /// Implementations may fail if their backing store errors.
    fn load(&self, plaintext_hash_hex: &str) -> Result<Option<Vec<u8>>, String>;

    /// Store a blob's plaintext under its plaintext hash (idempotent).
    ///
    /// # Errors
    /// Implementations may fail if their backing store errors.
    fn store(&self, plaintext_hash_hex: &str, plaintext: &[u8]) -> Result<(), String>;
}

/// An in-memory [`BlobStore`], primarily for tests and ephemeral use.
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    inner: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryBlobStore {
    /// Create an empty in-memory blob store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of blobs currently held.
    ///
    /// # Panics
    /// Panics only if the internal lock is poisoned by a prior panic.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |m| m.len())
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl BlobStore for MemoryBlobStore {
    fn load(&self, plaintext_hash_hex: &str) -> Result<Option<Vec<u8>>, String> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| "blob store poisoned".to_owned())?;
        Ok(guard.get(plaintext_hash_hex).cloned())
    }

    fn store(&self, plaintext_hash_hex: &str, plaintext: &[u8]) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "blob store poisoned".to_owned())?;
        guard.insert(plaintext_hash_hex.to_owned(), plaintext.to_vec());
        Ok(())
    }
}
