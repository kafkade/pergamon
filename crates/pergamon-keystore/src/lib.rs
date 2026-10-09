// SPDX-License-Identifier: Apache-2.0

//! # pergamon-keystore
//!
//! Device-key storage for pergamon sync (ADR-024, #125).
//!
//! Sync private keys — each device's X25519 + Ed25519 secrets and the Account
//! Root Key — never leave the machine and must be kept in a platform secure
//! store. This crate wraps two backends behind one [`DeviceKeyStore`]:
//!
//! - [`Backend::Keyring`] — the OS keychain (macOS Keychain, Linux Secret
//!   Service, Windows Credential Manager) via the `keyring` crate; the default
//!   on a normal desktop (the `keyring` feature, on by default), and
//! - `Backend::EncryptedFile` — an Argon2id-encrypted key file, the fallback
//!   for headless hosts without a live keychain (ADR-024's stated fallback) and
//!   the mechanism the AGPL web server uses to unlock keys for background sync
//!   (#129). Always available, so a service can depend on this crate with
//!   `default-features = false` to avoid linking the OS keychain.
//!
//! Only storage wiring lives here; the enroll/recover UX lives in the CLI.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pergamon_crypto::device::DeviceKeypairs;
use pergamon_crypto::hierarchy::{ACCOUNT_ID_LEN, AccountId, AccountRootKey};
use pergamon_crypto::primitives::{self, KEY_LEN};
use serde::{Deserialize, Serialize};

/// The `keyring` service name all pergamon secrets are stored under.
#[cfg(feature = "keyring")]
const KEYRING_SERVICE: &str = "dev.pergamon.sync";

/// Suffix identifying a device-secret entry (`{account}` scoped).
const DEVICE_SUFFIX: &str = "device";
/// Suffix identifying the Account Root Key entry (`{account}` scoped).
const ARK_SUFFIX: &str = "ark";
/// Suffix identifying the opaque ADR-022 account handle (`{account}` scoped).
const ACCOUNT_ID_SUFFIX: &str = "account-id";

/// A device-key store backed by either the OS keychain or an encrypted file.
pub struct DeviceKeyStore {
    backend: Backend,
}

/// Which secure store backs a [`DeviceKeyStore`].
enum Backend {
    /// The OS keychain via the `keyring` crate.
    #[cfg(feature = "keyring")]
    Keyring,
    /// An Argon2id-encrypted key file for headless hosts.
    EncryptedFile(EncryptedFile),
}

impl DeviceKeyStore {
    /// Use the OS keychain (macOS Keychain / Linux Secret Service / Windows
    /// Credential Manager). Requires the `keyring` feature.
    #[cfg(feature = "keyring")]
    #[must_use]
    pub const fn keyring() -> Self {
        Self {
            backend: Backend::Keyring,
        }
    }

    /// Use an Argon2id-encrypted key file at `path`, unlocked by `passphrase`.
    ///
    /// Loads the existing file if present, otherwise starts an empty store that
    /// is written on the first save.
    ///
    /// # Errors
    /// Returns an error if an existing file cannot be read or decrypted (e.g. a
    /// wrong passphrase).
    pub fn encrypted_file(path: impl Into<PathBuf>, passphrase: &[u8]) -> Result<Self> {
        let file = EncryptedFile::open(path.into(), passphrase)?;
        Ok(Self {
            backend: Backend::EncryptedFile(file),
        })
    }

    /// Persist a device's two secret scalars for `account`.
    ///
    /// # Errors
    /// Returns an error if the backing store rejects the write.
    pub fn save_device_keys(&mut self, account: &str, keys: &DeviceKeypairs) -> Result<()> {
        let mut blob = Vec::with_capacity(KEY_LEN * 2);
        blob.extend_from_slice(keys.x25519_secret());
        blob.extend_from_slice(keys.ed25519_signing());
        self.set(account, DEVICE_SUFFIX, &blob)
    }

    /// Load a device's keypairs for `account`, or `None` if none are stored.
    ///
    /// # Errors
    /// Returns an error if the store read fails or the stored blob is malformed.
    pub fn load_device_keys(&self, account: &str) -> Result<Option<DeviceKeypairs>> {
        let Some(blob) = self.get(account, DEVICE_SUFFIX)? else {
            return Ok(None);
        };
        if blob.len() != KEY_LEN * 2 {
            bail!(
                "stored device key blob has unexpected length {}",
                blob.len()
            );
        }
        let mut x = [0u8; KEY_LEN];
        let mut e = [0u8; KEY_LEN];
        x.copy_from_slice(&blob[..KEY_LEN]);
        e.copy_from_slice(&blob[KEY_LEN..]);
        Ok(Some(DeviceKeypairs::from_secrets(x, e)))
    }

    /// Persist the Account Root Key for `account`.
    ///
    /// # Errors
    /// Returns an error if the backing store rejects the write.
    pub fn save_ark(&mut self, account: &str, ark: &AccountRootKey) -> Result<()> {
        self.set(account, ARK_SUFFIX, ark.expose_bytes())
    }

    /// Load the Account Root Key for `account`, or `None` if none is stored.
    ///
    /// # Errors
    /// Returns an error if the store read fails or the stored blob is malformed.
    pub fn load_ark(&self, account: &str) -> Result<Option<AccountRootKey>> {
        let Some(blob) = self.get(account, ARK_SUFFIX)? else {
            return Ok(None);
        };
        let bytes: [u8; KEY_LEN] = blob
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("stored ARK has unexpected length {}", blob.len()))?;
        Ok(Some(AccountRootKey::from_bytes(bytes)))
    }

    /// Persist the opaque ADR-022 account handle for `account`.
    ///
    /// This is the wire `account_id` all of the account's devices share; the
    /// first device generates it and enrollment (#35) distributes it to others.
    ///
    /// # Errors
    /// Returns an error if the backing store rejects the write.
    pub fn save_account_id(&mut self, account: &str, id: &AccountId) -> Result<()> {
        self.set(account, ACCOUNT_ID_SUFFIX, id.as_bytes())
    }

    /// Load the opaque account handle for `account`, or `None` if unset.
    ///
    /// # Errors
    /// Returns an error if the store read fails or the stored blob is malformed.
    pub fn load_account_id(&self, account: &str) -> Result<Option<AccountId>> {
        let Some(blob) = self.get(account, ACCOUNT_ID_SUFFIX)? else {
            return Ok(None);
        };
        let bytes: [u8; ACCOUNT_ID_LEN] = blob.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!("stored account id has unexpected length {}", blob.len())
        })?;
        Ok(Some(AccountId::from_bytes(bytes)))
    }

    /// Atomically save a fresh account in an encrypted file before remote effects.
    ///
    /// # Errors
    /// Returns an error if any existing identity would be replaced, or the write fails.
    pub fn save_account_material(
        &mut self,
        account: &str,
        keys: &DeviceKeypairs,
        ark: &AccountRootKey,
        id: &AccountId,
    ) -> Result<()> {
        if self.load_device_keys(account)?.is_some_and(|old| {
            old.x25519_secret() != keys.x25519_secret()
                || old.ed25519_signing() != keys.ed25519_signing()
        }) || self
            .load_ark(account)?
            .is_some_and(|old| old.expose_bytes() != ark.expose_bytes())
            || self.load_account_id(account)?.is_some_and(|old| old != *id)
        {
            bail!("existing account material must not be replaced");
        }
        let mut device = zeroize::Zeroizing::new(Vec::with_capacity(KEY_LEN * 2));
        device.extend_from_slice(keys.x25519_secret());
        device.extend_from_slice(keys.ed25519_signing());
        let entries = [
            (entry_name(account, DEVICE_SUFFIX), Some(device.as_slice())),
            (
                entry_name(account, ARK_SUFFIX),
                Some(ark.expose_bytes().as_slice()),
            ),
            (
                entry_name(account, ACCOUNT_ID_SUFFIX),
                Some(id.as_bytes().as_slice()),
            ),
        ];
        match &mut self.backend {
            #[cfg(feature = "keyring")]
            Backend::Keyring => {
                bail!("atomic account creation requires the encrypted-file backend")
            }
            Backend::EncryptedFile(file) => file.update(&entries),
        }
    }

    /// Persist a redacted-on-output session bundle separately from account keys.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn save_remote_session(&mut self, account: &str, relay: &str, bytes: &[u8]) -> Result<()> {
        self.set(account, &session_suffix(relay), bytes)
    }

    /// Load session bytes from an unlocked store; decoding belongs to the client.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn load_remote_session(&self, account: &str, relay: &str) -> Result<Option<Vec<u8>>> {
        self.get(account, &session_suffix(relay))
    }

    /// Keep a first-device recovery artifact stable across interrupted publication.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn save_bootstrap_recovery(
        &mut self,
        account: &str,
        relay: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.set(
            account,
            &format!("bootstrap-{}", session_suffix(relay)),
            bytes,
        )
    }

    /// Load an unpublished/retried recovery artifact from the unlocked store.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn load_bootstrap_recovery(&self, account: &str, relay: &str) -> Result<Option<Vec<u8>>> {
        self.get(account, &format!("bootstrap-{}", session_suffix(relay)))
    }

    /// Delete acknowledged capture material without deleting account keys.
    ///
    /// # Errors
    /// Returns an error if secure-store deletion fails.
    pub fn remove_bootstrap_recovery(&mut self, account: &str, relay: &str) -> Result<()> {
        let entry = entry_name(account, &format!("bootstrap-{}", session_suffix(relay)));
        match &mut self.backend {
            #[cfg(feature = "keyring")]
            Backend::Keyring => {
                let item = keyring::Entry::new(KEYRING_SERVICE, &entry)?;
                match item.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(e) => Err(e.into()),
                }
            }
            Backend::EncryptedFile(file) => file.update(&[(entry, None)]),
        }
    }

    /// Persist exact sealed onboarding publications for idempotent retries.
    ///
    /// # Errors
    /// Returns an error if encrypted persistence fails.
    pub fn save_onboarding_artifact(
        &mut self,
        account: &str,
        relay: &str,
        operation: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.set(
            account,
            &format!(
                "onboarding-{}-{}",
                session_suffix(relay),
                session_suffix(operation)
            ),
            bytes,
        )
    }

    /// Load an exact publication without regenerating its ciphertext.
    ///
    /// # Errors
    /// Returns an error on corrupt or unreadable storage.
    pub fn load_onboarding_artifact(
        &self,
        account: &str,
        relay: &str,
        operation: &str,
    ) -> Result<Option<Vec<u8>>> {
        self.get(
            account,
            &format!(
                "onboarding-{}-{}",
                session_suffix(relay),
                session_suffix(operation)
            ),
        )
    }

    /// Write a secret under the `{account}:{suffix}` entry.
    fn set(&mut self, account: &str, suffix: &str, bytes: &[u8]) -> Result<()> {
        let entry = entry_name(account, suffix);
        match &mut self.backend {
            #[cfg(feature = "keyring")]
            Backend::Keyring => keyring_set(&entry, bytes),
            Backend::EncryptedFile(file) => file.set(&entry, bytes),
        }
    }

    /// Read a secret from the `{account}:{suffix}` entry, if present.
    #[cfg_attr(not(feature = "keyring"), allow(clippy::unnecessary_wraps))]
    fn get(&self, account: &str, suffix: &str) -> Result<Option<Vec<u8>>> {
        let entry = entry_name(account, suffix);
        match &self.backend {
            #[cfg(feature = "keyring")]
            Backend::Keyring => keyring_get(&entry),
            Backend::EncryptedFile(file) => file.get(&entry),
        }
    }
}

/// The keychain account name for a scoped secret.
fn entry_name(account: &str, suffix: &str) -> String {
    format!("{account}:{suffix}")
}

fn session_suffix(relay: &str) -> String {
    use std::fmt::Write as _;
    let hash = primitives::blake3_hash(relay.trim_end_matches('/').as_bytes());
    let mut suffix = String::from("remote-session-");
    for byte in hash {
        let _ = write!(suffix, "{byte:02x}");
    }
    suffix
}

/// Store `bytes` (base64) in the OS keychain under `entry`.
#[cfg(feature = "keyring")]
fn keyring_set(entry: &str, bytes: &[u8]) -> Result<()> {
    let item = keyring::Entry::new(KEYRING_SERVICE, entry)
        .with_context(|| format!("opening keychain entry {entry}"))?;
    item.set_password(&STANDARD.encode(bytes))
        .with_context(|| format!("writing keychain entry {entry}"))?;
    Ok(())
}

/// Read `bytes` from the OS keychain under `entry`, if present.
#[cfg(feature = "keyring")]
fn keyring_get(entry: &str) -> Result<Option<Vec<u8>>> {
    let item = keyring::Entry::new(KEYRING_SERVICE, entry)
        .with_context(|| format!("opening keychain entry {entry}"))?;
    match item.get_password() {
        Ok(encoded) => {
            let bytes = STANDARD
                .decode(encoded.as_bytes())
                .with_context(|| format!("decoding keychain entry {entry}"))?;
            Ok(Some(bytes))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context(format!("reading keychain entry {entry}"))),
    }
}

/// On-disk JSON layout of the encrypted key file.
#[derive(Serialize, Deserialize)]
struct FileFormat {
    /// Base64 per-file Argon2id salt.
    salt_b64: String,
    /// Entry name -> base64 `aead_seal(kek, aad=entry, secret)` (`nonce‖ct‖tag`).
    entries: BTreeMap<String, String>,
}

/// An Argon2id-encrypted key file plus its decrypted key-encryption key.
struct EncryptedFile {
    path: PathBuf,
    salt: [u8; SALT_LEN],
    kek: primitives::SymmetricKey,
    entries: BTreeMap<String, String>,
}

/// Length of the per-file Argon2id salt.
const SALT_LEN: usize = 16;

impl EncryptedFile {
    /// Open an existing key file (verifying the passphrase against any stored
    /// entry) or prepare a fresh in-memory store to be written on first save.
    fn open(path: PathBuf, passphrase: &[u8]) -> Result<Self> {
        if path.exists() {
            let raw = std::fs::read(&path)
                .with_context(|| format!("reading key file {}", path.display()))?;
            let parsed: FileFormat = serde_json::from_slice(&raw)
                .with_context(|| format!("parsing key file {}", path.display()))?;
            let salt = decode_salt(&parsed.salt_b64)?;
            let kek = primitives::argon2id_kek(passphrase, &salt)
                .context("deriving key-encryption key")?;
            let file = Self {
                path,
                salt,
                kek,
                entries: parsed.entries,
            };
            // Verify the passphrase eagerly by decrypting one entry, if any.
            if let Some((name, _)) = file.entries.iter().next() {
                file.decrypt_entry(name)
                    .context("wrong passphrase or corrupt key file")?;
            }
            Ok(file)
        } else {
            let salt = primitives::random_array::<SALT_LEN>().context("generating salt")?;
            let kek = primitives::argon2id_kek(passphrase, &salt)
                .context("deriving key-encryption key")?;
            Ok(Self {
                path,
                salt,
                kek,
                entries: BTreeMap::new(),
            })
        }
    }

    /// Encrypt and store `bytes` under `entry`, then flush the file.
    fn set(&mut self, entry: &str, bytes: &[u8]) -> Result<()> {
        self.update(&[(entry.to_owned(), Some(bytes))])
    }

    fn update(&mut self, entries: &[(String, Option<&[u8]>)]) -> Result<()> {
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).context("creating key-file directory")?;
        }
        let lock_path = self.path.with_extension("write-lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let lock = options
            .open(lock_path)
            .context("opening key-file writer lock")?;
        lock.lock().context("locking secure-store writes")?;
        if self.path.exists() {
            let raw =
                std::fs::read(&self.path).context("refreshing secure key file before write")?;
            let current: FileFormat =
                serde_json::from_slice(&raw).context("decoding current secure key file")?;
            if decode_salt(&current.salt_b64)? != self.salt {
                bail!("secure key file was replaced; unlock it again before writing");
            }
            self.entries = current.entries;
        }
        for (entry, bytes) in entries {
            if let Some(bytes) = bytes {
                let sealed = primitives::aead_seal(&self.kek, entry.as_bytes(), bytes)
                    .context("sealing key-file entry")?;
                self.entries.insert(entry.clone(), STANDARD.encode(&sealed));
            } else {
                self.entries.remove(entry);
            }
        }
        self.flush()
    }

    /// Decrypt and return the secret under `entry`, if present.
    fn get(&self, entry: &str) -> Result<Option<Vec<u8>>> {
        self.decrypt_entry(entry)
    }

    /// Decrypt the named entry, returning `Ok(None)` when it is absent and an
    /// error when decryption fails (wrong passphrase / tampering).
    fn decrypt_entry(&self, entry: &str) -> Result<Option<Vec<u8>>> {
        let Some(encoded) = self.entries.get(entry) else {
            return Ok(None);
        };
        let sealed = STANDARD
            .decode(encoded.as_bytes())
            .context("decoding key-file entry")?;
        let plain = primitives::aead_open(&self.kek, entry.as_bytes(), &sealed)
            .map_err(|e| anyhow::anyhow!("decrypting key-file entry {entry}: {e}"))?;
        Ok(Some(plain))
    }

    /// Serialize and atomically write the key file.
    fn flush(&self) -> Result<()> {
        use std::io::Write as _;
        let format = FileFormat {
            salt_b64: STANDARD.encode(self.salt),
            entries: self.entries.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&format).context("serializing key file")?;
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating key-file directory {}", parent.display()))?;
        }
        let nonce = primitives::random_array::<16>().context("generating key-file write nonce")?;
        let tmp = self.path.with_extension(format!(
            "tmp-{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let result = (|| -> Result<()> {
            let mut file = options
                .open(&tmp)
                .context("creating private key-file temporary")?;
            file.write_all(&bytes)
                .context("writing encrypted key file")?;
            file.sync_all().context("flushing encrypted key file")?;
            drop(file);
            std::fs::rename(&tmp, &self.path).context("installing encrypted key file")?;
            #[cfg(unix)]
            if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::File::open(parent)?
                    .sync_all()
                    .context("flushing key-file directory")?;
            }
            Ok(())
        })();
        if result.is_err() {
            match std::fs::remove_file(&tmp) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        result?;
        Ok(())
    }
}

/// Decode and length-check the stored Argon2id salt.
fn decode_salt(b64: &str) -> Result<[u8; SALT_LEN]> {
    let raw = STANDARD.decode(b64.as_bytes()).context("decoding salt")?;
    raw.as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("stored salt has unexpected length {}", raw.len()))
}

/// The default key-file path for the encrypted-file fallback.
#[must_use]
pub fn default_key_file(config_dir: &Path) -> PathBuf {
    config_dir.join("sync-keys.json")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("pergamon-keystore-{}.json", uuid::Uuid::new_v4()))
    }

    fn cleanup(path: &Path) {
        for file in [path.to_owned(), path.with_extension("write-lock")] {
            std::fs::remove_file(file)
                .or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .unwrap();
        }
    }

    #[test]
    fn remote_session_is_scoped_encrypted_and_survives_other_key_writes() {
        let path = temp_path();
        let mut first = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
        first
            .save_ark("owner", &AccountRootKey::from_bytes([7; 32]))
            .unwrap();
        let mut stale = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
        let session=b"{\"access_token\":\"private-access-fixture\",\"refresh_token\":\"private-refresh-fixture\"}";
        first
            .save_remote_session("owner", "https://relay.example/", session)
            .unwrap();
        stale
            .save_account_id("owner", &AccountId::from_bytes([9; 16]))
            .unwrap();
        let reopened = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
        assert_eq!(
            reopened
                .load_remote_session("owner", "https://relay.example")
                .unwrap()
                .unwrap(),
            session
        );
        assert!(
            reopened
                .load_remote_session("other", "https://relay.example")
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .load_remote_session("owner", "https://elsewhere.example")
                .unwrap()
                .is_none()
        );
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(session.len()).any(|w| w == session));
        let mut parsed: FileFormat = serde_json::from_slice(&raw).unwrap();
        parsed.entries.insert(
            entry_name("owner", &session_suffix("https://relay.example")),
            "invalid base64".to_owned(),
        );
        std::fs::write(&path, serde_json::to_vec(&parsed).unwrap()).unwrap();
        let corrupt = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
        assert!(
            corrupt
                .load_remote_session("owner", "https://relay.example")
                .is_err()
        );
        cleanup(&path);
    }

    #[test]
    fn account_batch_and_capture_cleanup_preserve_existing_keys() {
        let path = temp_path();
        let keys = DeviceKeypairs::generate().unwrap();
        let ark = AccountRootKey::from_bytes([7; 32]);
        let id = AccountId::from_bytes([3; 16]);
        let mut store = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
        store
            .save_account_material("owner", &keys, &ark, &id)
            .unwrap();
        store
            .save_bootstrap_recovery("owner", "https://relay.example", b"capture-secret")
            .unwrap();
        store
            .remove_bootstrap_recovery("owner", "https://relay.example")
            .unwrap();
        let reopened = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
        assert_eq!(reopened.load_account_id("owner").unwrap(), Some(id.clone()));
        assert_eq!(
            reopened.load_ark("owner").unwrap().unwrap().expose_bytes(),
            ark.expose_bytes()
        );
        assert!(
            reopened
                .load_bootstrap_recovery("owner", "https://relay.example")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .save_account_material("owner", &keys, &AccountRootKey::from_bytes([8; 32]), &id)
                .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(store);
        drop(reopened);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(path.with_extension("write-lock")).unwrap();
    }

    #[test]
    fn encrypted_file_round_trips_device_and_ark() {
        let path = temp_path();
        let account = "acct-1";
        let keys = DeviceKeypairs::generate().unwrap();
        let ark = AccountRootKey::from_bytes([7u8; 32]);

        {
            let mut store = DeviceKeyStore::encrypted_file(&path, b"passphrase").unwrap();
            store.save_device_keys(account, &keys).unwrap();
            store.save_ark(account, &ark).unwrap();
        }

        // Re-open with the correct passphrase and read the secrets back.
        let store = DeviceKeyStore::encrypted_file(&path, b"passphrase").unwrap();
        let loaded = store.load_device_keys(account).unwrap().unwrap();
        assert_eq!(loaded.device_id(), keys.device_id());
        assert_eq!(loaded.x25519_secret(), keys.x25519_secret());
        assert_eq!(loaded.ed25519_signing(), keys.ed25519_signing());
        let loaded_ark = store.load_ark(account).unwrap().unwrap();
        assert_eq!(loaded_ark.expose_bytes(), ark.expose_bytes());

        cleanup(&path);
    }

    #[test]
    fn encrypted_file_rejects_wrong_passphrase() {
        let path = temp_path();
        let keys = DeviceKeypairs::generate().unwrap();
        {
            let mut store = DeviceKeyStore::encrypted_file(&path, b"right").unwrap();
            store.save_device_keys("acct", &keys).unwrap();
        }
        // Opening with the wrong passphrase fails because the probe entry can't
        // be decrypted.
        assert!(DeviceKeyStore::encrypted_file(&path, b"wrong").is_err());
        cleanup(&path);
    }

    #[test]
    fn missing_entries_return_none() {
        let path = temp_path();
        let store = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
        assert!(store.load_device_keys("nobody").unwrap().is_none());
        assert!(store.load_ark("nobody").unwrap().is_none());
        cleanup(&path);
    }

    #[test]
    fn ciphertext_file_holds_no_plaintext_secret() {
        let path = temp_path();
        let ark = AccountRootKey::from_bytes([0xab; 32]);
        {
            let mut store = DeviceKeyStore::encrypted_file(&path, b"pw").unwrap();
            store.save_ark("acct", &ark).unwrap();
        }
        let raw = std::fs::read(&path).unwrap();
        assert!(
            !raw.windows(32).any(|w| w == ark.expose_bytes()),
            "raw ARK bytes must not appear in the key file"
        );
        cleanup(&path);
    }
}
