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
use std::fs;
use std::path::PathBuf;
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

/// A durable, filesystem-backed [`BlobStore`] (issue #184).
///
/// Blob plaintext is content-addressed: each blob is one file named after its
/// lowercase-hex blake3 plaintext hash, directly under `root`. Writes are atomic
/// (write to a unique temp file in the same directory, then rename over the
/// target) so a crash mid-write never leaves a partial blob, and `store` is
/// idempotent (re-storing the same content is a harmless overwrite of identical
/// bytes). Unlike [`MemoryBlobStore`], the contents survive process restarts, so
/// blob plaintext saved on push/apply remains available to later invocations.
#[derive(Debug, Clone)]
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Open (creating if needed) a filesystem blob store rooted at `root`.
    ///
    /// # Errors
    /// Returns a message if the root directory cannot be created.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|e| format!("creating blob dir {}: {e}", root.display()))?;
        Ok(Self { root })
    }

    /// Reject hashes that are not plain lowercase-hex so a hash can never escape
    /// the store's directory (path traversal) or name an unexpected file.
    fn path_for(&self, plaintext_hash_hex: &str) -> Result<PathBuf, String> {
        let ok = !plaintext_hash_hex.is_empty()
            && plaintext_hash_hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !ok {
            return Err(format!("invalid blob hash `{plaintext_hash_hex}`"));
        }
        Ok(self.root.join(plaintext_hash_hex))
    }
}

impl BlobStore for FsBlobStore {
    fn load(&self, plaintext_hash_hex: &str) -> Result<Option<Vec<u8>>, String> {
        let path = self.path_for(plaintext_hash_hex)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("reading blob {}: {e}", path.display())),
        }
    }

    fn store(&self, plaintext_hash_hex: &str, plaintext: &[u8]) -> Result<(), String> {
        let path = self.path_for(plaintext_hash_hex)?;
        if path.exists() {
            return Ok(());
        }
        // Atomic write: a unique temp file in the same directory, then rename.
        let tmp = self.root.join(format!(
            "{plaintext_hash_hex}.tmp.{}.{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        fs::write(&tmp, plaintext)
            .map_err(|e| format!("writing blob temp {}: {e}", tmp.display()))?;
        match fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(format!("committing blob {}: {e}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn fs_blob_round_trips_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "0a1b2c";
        {
            let store = FsBlobStore::new(dir.path()).unwrap();
            assert_eq!(store.load(hash).unwrap(), None, "absent -> None");
            store.store(hash, b"hello").unwrap();
            assert_eq!(store.load(hash).unwrap(), Some(b"hello".to_vec()));
            // Idempotent: re-storing identical content is a no-op success.
            store.store(hash, b"hello").unwrap();
            assert_eq!(store.load(hash).unwrap(), Some(b"hello".to_vec()));
        }
        // A fresh instance pointed at the same dir still sees the blob.
        let reopened = FsBlobStore::new(dir.path()).unwrap();
        assert_eq!(reopened.load(hash).unwrap(), Some(b"hello".to_vec()));
    }

    #[test]
    fn fs_blob_rejects_non_hex_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        assert!(store.load("../escape").is_err());
        assert!(store.store("Znothex", b"x").is_err());
    }
}
