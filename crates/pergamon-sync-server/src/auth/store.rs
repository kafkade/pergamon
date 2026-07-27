// SPDX-License-Identifier: AGPL-3.0-only

//! The OPAQUE auth store — a **separate** `SQLite` database from the blind
//! relay's content store (design §1.5, §2.3).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! This store holds:
//! - [`accounts`] — the OPAQUE registration record (a **verifier only**, never a
//!   password or password-equivalent) keyed by `identity_handle`, plus the
//!   `oprf_key_id` it was created under (for future rotation, design §1.8).
//! - [`account_map`] — the internal `identity_handle → account_id` mapping. The
//!   opaque `account_id` is what the blind content routes key on; it is never
//!   exposed to unauthenticated callers.
//! - [`auth_failures`] — per-identity throttling counters (design §1.7).
//!
//! The OPRF server secret ([`opaque_ke::ServerSetup`]) is **not** stored here: it
//! lives outside the verifier database (a separate file loaded at startup), so a
//! stolen `accounts` table does not also yield the OPRF key that gates offline
//! guessing (design §1.8).
//!
//! [`accounts`]: AuthStore
//! [`account_map`]: AuthStore
//! [`auth_failures`]: AuthStore

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::throttle::ThrottleConfig;

/// Errors returned by the [`AuthStore`].
#[derive(Debug, thiserror::Error)]
pub enum AuthStoreError {
    /// An account already exists for this `identity_handle`.
    #[error("an account already exists for this identity")]
    HandleExists,

    /// An underlying database error.
    #[error("auth database error: {0}")]
    Db(#[from] rusqlite::Error),
}

/// Current epoch time in milliseconds.
fn now_ms() -> i64 {
    // `unix_timestamp_nanos` is i128; ms fits comfortably in i64 for any real
    // date. `try_from` saturates rather than panics on an implausible clock.
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

/// The current throttling state for one identity.
#[derive(Debug, Clone, Copy)]
pub struct FailureState {
    /// Consecutive failure count.
    pub failure_count: u32,
    /// Epoch-millis until which logins are locked out (0 if never).
    pub locked_until_ms: i64,
}

/// A separate `SQLite` store for OPAQUE verifiers, handle→account mapping, and
/// per-identity throttling. Never co-mingled with [`crate::store::SyncStore`].
pub struct AuthStore {
    conn: Connection,
}

impl AuthStore {
    /// Open (creating if needed) a file-backed auth store and initialize schema.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] if the database cannot be opened/migrated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuthStoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open an in-memory auth store (used by tests).
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] if the schema cannot be created.
    pub fn open_in_memory() -> Result<Self, AuthStoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Create the auth tables if they do not yet exist.
    fn init_schema(&self) -> Result<(), AuthStoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS accounts (
                identity_handle TEXT    NOT NULL PRIMARY KEY,
                -- ServerRegistration::serialize() — the OPAQUE verifier/envelope
                -- ONLY (client public key + masking key + envelope). Never a
                -- password or password-equivalent.
                opaque_record   BLOB    NOT NULL,
                oprf_key_id     TEXT    NOT NULL,
                created_at      INTEGER NOT NULL
            );

            -- Internal login-handle → opaque content-plane account_id map. The
            -- account_id is decoupled from the login handle and is what the
            -- blind content routes key on. Never exposed to unauthenticated
            -- callers.
            CREATE TABLE IF NOT EXISTS account_map (
                identity_handle TEXT NOT NULL PRIMARY KEY,
                account_id      TEXT NOT NULL UNIQUE
            );

            -- Per-identity online-guessing throttle counters (design §1.7).
            -- Keyed on identity_handle uniformly whether or not an account
            -- exists, so lockout is not an account-existence oracle.
            CREATE TABLE IF NOT EXISTS auth_failures (
                identity_handle TEXT    NOT NULL PRIMARY KEY,
                failure_count   INTEGER NOT NULL DEFAULT 0,
                first_failed_at INTEGER,
                last_failed_at  INTEGER,
                locked_until    INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        Ok(())
    }

    /// Return `true` if an account is registered for `identity_handle`.
    fn account_exists(&self, identity_handle: &str) -> Result<bool, AuthStoreError> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM accounts WHERE identity_handle = ?1",
                params![identity_handle],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Finalize registration: persist the verifier and allocate a random opaque
    /// `account_id`, atomically. Returns the allocated `account_id`.
    ///
    /// The `account_id` is a fresh random 128-bit handle (design §1.6, ADR-024:
    /// an independent random handle, not derived from any password). Reconciling
    /// this server-allocated id with a client-generated ADR-024 `account_id` at
    /// device-attach time is a follow-up (WP-3b/#192); WP-3a allocates it here.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::HandleExists`] if the handle is already
    /// registered, or [`AuthStoreError::Db`] on a database failure.
    pub fn finish_registration(
        &mut self,
        identity_handle: &str,
        opaque_record: &[u8],
        oprf_key_id: &str,
    ) -> Result<String, AuthStoreError> {
        if self.account_exists(identity_handle)? {
            return Err(AuthStoreError::HandleExists);
        }
        let account_id = Uuid::new_v4().simple().to_string();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO accounts (identity_handle, opaque_record, oprf_key_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![identity_handle, opaque_record, oprf_key_id, now_ms()],
        )?;
        tx.execute(
            "INSERT INTO account_map (identity_handle, account_id) VALUES (?1, ?2)",
            params![identity_handle, account_id],
        )?;
        tx.commit()?;
        Ok(account_id)
    }

    /// Fetch the stored OPAQUE verifier for `identity_handle`, if registered.
    ///
    /// A `None` result drives the privacy-preserving dummy-login path
    /// ([`opaque_ke::ServerLogin::start`] with `None`), so an unknown identity is
    /// indistinguishable from a wrong-password attempt (design §1.6).
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn opaque_record(&self, identity_handle: &str) -> Result<Option<Vec<u8>>, AuthStoreError> {
        let record = self
            .conn
            .query_row(
                "SELECT opaque_record FROM accounts WHERE identity_handle = ?1",
                params![identity_handle],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        Ok(record)
    }

    /// Look up the opaque `account_id` for a (now authenticated) identity.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn account_id(&self, identity_handle: &str) -> Result<Option<String>, AuthStoreError> {
        let account_id = self
            .conn
            .query_row(
                "SELECT account_id FROM account_map WHERE identity_handle = ?1",
                params![identity_handle],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(account_id)
    }

    /// Read the current throttle state for an identity (default zero state).
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn failure_state(&self, identity_handle: &str) -> Result<FailureState, AuthStoreError> {
        let state = self
            .conn
            .query_row(
                "SELECT failure_count, locked_until FROM auth_failures WHERE identity_handle = ?1",
                params![identity_handle],
                |row| {
                    Ok(FailureState {
                        failure_count: u32::try_from(row.get::<_, i64>(0)?.max(0))
                            .unwrap_or(u32::MAX),
                        locked_until_ms: row.get::<_, i64>(1)?,
                    })
                },
            )
            .optional()?;
        Ok(state.unwrap_or(FailureState {
            failure_count: 0,
            locked_until_ms: 0,
        }))
    }

    /// Return the lockout expiry (epoch millis) if the identity is currently
    /// locked out, else `None`.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn locked_until(&self, identity_handle: &str) -> Result<Option<i64>, AuthStoreError> {
        let state = self.failure_state(identity_handle)?;
        if state.locked_until_ms > now_ms() {
            Ok(Some(state.locked_until_ms))
        } else {
            Ok(None)
        }
    }

    /// Record a failed login for an identity and (re)compute its lockout.
    ///
    /// Counters are keyed on `identity_handle` uniformly, whether or not an
    /// account exists, so this cannot leak account existence (design §1.6).
    ///
    /// Returns the resulting [`FailureState`].
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn record_failure(
        &mut self,
        identity_handle: &str,
        throttle: &ThrottleConfig,
    ) -> Result<FailureState, AuthStoreError> {
        let now = now_ms();
        let current = self.failure_state(identity_handle)?;
        let new_count = current.failure_count.saturating_add(1);
        let lockout_ms =
            i64::try_from(throttle.lockout_for(new_count).as_millis()).unwrap_or(i64::MAX);
        let locked_until = now.saturating_add(lockout_ms);
        self.conn.execute(
            "INSERT INTO auth_failures
                 (identity_handle, failure_count, first_failed_at, last_failed_at, locked_until)
             VALUES (?1, 1, ?2, ?2, ?3)
             ON CONFLICT(identity_handle) DO UPDATE SET
                 failure_count = failure_count + 1,
                 last_failed_at = ?2,
                 locked_until = ?3",
            params![identity_handle, now, locked_until],
        )?;
        Ok(FailureState {
            failure_count: new_count,
            locked_until_ms: locked_until,
        })
    }

    /// Clear the failure counter for an identity after a successful login.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn reset_failures(&mut self, identity_handle: &str) -> Result<(), AuthStoreError> {
        self.conn.execute(
            "DELETE FROM auth_failures WHERE identity_handle = ?1",
            params![identity_handle],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn registration_stores_verifier_and_allocates_account_id() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let record = b"opaque-verifier-bytes".to_vec();
        let account_id = store.finish_registration("alice", &record, "v1").unwrap();
        assert!(!account_id.is_empty());
        assert_eq!(store.opaque_record("alice").unwrap(), Some(record));
        assert_eq!(store.account_id("alice").unwrap(), Some(account_id));
        assert_eq!(store.opaque_record("bob").unwrap(), None);
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut store = AuthStore::open_in_memory().unwrap();
        store.finish_registration("alice", b"rec", "v1").unwrap();
        let err = store
            .finish_registration("alice", b"rec2", "v1")
            .unwrap_err();
        assert!(matches!(err, AuthStoreError::HandleExists));
    }

    #[test]
    fn failure_counter_locks_after_threshold_and_resets() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let cfg = ThrottleConfig::default();
        assert!(store.locked_until("mallory").unwrap().is_none());
        // Below threshold: no lock yet.
        for _ in 0..cfg.threshold {
            store.record_failure("mallory", &cfg).unwrap();
        }
        assert!(store.locked_until("mallory").unwrap().is_none());
        // One past threshold: locked.
        let state = store.record_failure("mallory", &cfg).unwrap();
        assert_eq!(state.failure_count, cfg.threshold + 1);
        assert!(store.locked_until("mallory").unwrap().is_some());
        // Reset clears the lock.
        store.reset_failures("mallory").unwrap();
        assert!(store.locked_until("mallory").unwrap().is_none());
    }

    #[test]
    fn failure_counter_is_keyed_per_identity() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let cfg = ThrottleConfig::default();
        for _ in 0..(cfg.threshold + 3) {
            store.record_failure("locked-one", &cfg).unwrap();
        }
        assert!(store.locked_until("locked-one").unwrap().is_some());
        // A different (never-tried) identity is unaffected — but note lockout
        // for a *nonexistent* handle behaves identically to an existent one.
        assert!(store.locked_until("fresh-one").unwrap().is_none());
    }
}
