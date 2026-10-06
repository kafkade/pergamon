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
//! - [`account_map`] — the internal `identity_handle → auth_tenant_id` mapping.
//!   Content routes resolve that tenant's separate canonical binding; only
//!   legacy reserved bindings retain equal tenant/content IDs.
//! - [`auth_failures`] — per-identity throttling counters (design §1.7).
//! - [`tokens`] — per-device bearer/refresh tokens minted after a successful
//!   login, bound to the device's ADR-024 Ed25519 key (WP-3b, #192). Only a
//!   hash of each token secret is stored, never the secret itself.
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
use crate::auth::token::{self, AuthAccount, TokenKind};

/// Errors returned by the [`AuthStore`].
#[derive(Debug, thiserror::Error)]
pub enum AuthStoreError {
    /// An account already exists for this `identity_handle`.
    #[error("an account already exists for this identity")]
    HandleExists,

    /// A single-use credential was already consumed or expired.
    #[error("invalid or expired token")]
    InvalidToken,

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

/// A bearer/refresh token that passed validation: it exists, is of the expected
/// kind, is unexpired, is not revoked, and its presented secret hashes to the
/// stored `token_hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedToken {
    /// The token's stable row id (revocation handle; also the refresh-PoP input).
    pub token_id: String,
    /// The single account this token authorizes.
    pub auth_tenant_id: String,
    /// The ADR-024 device the token is bound to.
    pub device_id: String,
    /// The device Ed25519 key the token is bound to (proof-of-possession target).
    pub ed25519_pub: [u8; token::ED25519_PUB_LEN],
    /// Canonical content identity, absent for an unbound control-plane token.
    pub content_account_id: Option<String>,
    /// Immutable authority snapshot at issuance.
    pub binding_version: i64,
    /// Control-only or content authority, validated against the identity snapshot.
    pub scope: token::TokenScope,
}

/// The result of a refresh-token rotation: a fresh access token and a fresh
/// refresh token.
///
/// The presented refresh token is revoked in the same transaction, so each
/// refresh secret is single-use (WP-3b hardening).
#[derive(Debug, Clone)]
pub struct RotatedTokens {
    /// The new opaque access-token bearer string.
    pub access_token: String,
    /// The new access token's expiry, epoch milliseconds.
    pub access_expires_at: i64,
    /// The new opaque refresh-token bearer string (replaces the presented one).
    pub refresh_token: String,
    /// The new refresh token's expiry, epoch milliseconds.
    pub refresh_expires_at: i64,
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
            );

            -- Per-device bearer/refresh tokens (WP-3b, #192). Each row is scoped
            -- to exactly one account_id and bound to a device's ADR-024 Ed25519
            -- key (proof-of-possession at mint/refresh). token_hash is
            -- blake3(secret); the raw secret is NEVER stored, so a theft of this
            -- table yields no usable bearer tokens. revoked_at, when set, rejects
            -- the token on validation and refresh (server-auth revocation,
            -- independent of content-plane epoch rotation — ADR-024/ADR-030).
            CREATE TABLE IF NOT EXISTS tokens (
                token_id    TEXT    NOT NULL PRIMARY KEY,
                account_id  TEXT    NOT NULL,
                device_id   TEXT    NOT NULL,
                ed25519_pub BLOB    NOT NULL,
                token_hash  BLOB    NOT NULL,
                kind        TEXT    NOT NULL,
                created_at  INTEGER NOT NULL,
                expires_at  INTEGER NOT NULL,
                revoked_at  INTEGER
            );
            ",
        )?;
        crate::auth::binding::migrate(&self.conn)?;
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tokens_device ON tokens(auth_tenant_id,device_id);
             PRAGMA foreign_keys = ON;",
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
    /// This legacy API reserves content ID equal to the newly allocated auth
    /// tenant. Official relay routes check namespace history under the writer
    /// barrier first. Existing local accounts instead use v2 unbound
    /// registration and explicit binding; their content IDs are never replaced.
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
        self.finish_registration_at(
            identity_handle,
            opaque_record,
            oprf_key_id,
            &Uuid::new_v4().simple().to_string(),
        )
    }

    pub(crate) fn finish_registration_at(
        &mut self,
        identity_handle: &str,
        opaque_record: &[u8],
        oprf_key_id: &str,
        account_id: &str,
    ) -> Result<String, AuthStoreError> {
        if self.account_exists(identity_handle)? {
            return Err(AuthStoreError::HandleExists);
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO accounts (identity_handle, opaque_record, oprf_key_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![identity_handle, opaque_record, oprf_key_id, now_ms()],
        )?;
        tx.execute(
            "INSERT INTO account_map (identity_handle, auth_tenant_id) VALUES (?1, ?2)",
            params![identity_handle, account_id],
        )?;
        tx.execute(
            "INSERT INTO content_bindings (content_account_id, auth_tenant_id, state, binding_version)
             VALUES (?1, ?1, 'legacy_reserved', 0)",
            params![account_id],
        )?;
        tx.commit()?;
        Ok(account_id.to_owned())
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

    /// Look up the authentication tenant for a proved identity.
    ///
    /// This legacy Rust method name does not mean canonical content identity;
    /// callers must resolve [`Self::binding`] separately.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn account_id(&self, identity_handle: &str) -> Result<Option<String>, AuthStoreError> {
        let account_id = self
            .conn
            .query_row(
                "SELECT auth_tenant_id FROM account_map WHERE identity_handle = ?1",
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

    // --- Per-device tokens (WP-3b, #192) -------------------------------------

    /// Insert one token row on any connection (or transaction). Shared by
    /// [`Self::insert_token`] and [`Self::rotate_refresh`]. Only the token
    /// **hash** is stored, never the secret.
    #[allow(clippy::too_many_arguments)]
    fn insert_token_row(
        conn: &Connection,
        token_id: &str,
        account_id: &str,
        device_id: &str,
        ed25519_pub: &[u8],
        token_hash: &[u8],
        kind: TokenKind,
        expires_at_ms: i64,
    ) -> Result<(), AuthStoreError> {
        let binding = crate::auth::binding::binding_for(conn, account_id)?;
        conn.execute(
            "INSERT INTO tokens
                 (token_id, auth_tenant_id, device_id, ed25519_pub, token_hash, kind,
                  created_at, expires_at, revoked_at, content_account_id, binding_version, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11)",
            params![
                token_id,
                account_id,
                device_id,
                ed25519_pub,
                token_hash,
                kind.as_str(),
                now_ms(),
                expires_at_ms,
                binding.as_ref().map(|b| &b.content_account_id),
                binding.as_ref().map_or(0, |b| b.binding_version),
                if binding.is_some() {
                    "content"
                } else {
                    "control"
                },
            ],
        )?;
        Ok(())
    }

    /// Persist a freshly minted token bound to `(account_id, device_id,
    /// ed25519_pub)`. Only the token **hash** is stored, never the secret.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_token(
        &mut self,
        token_id: &str,
        account_id: &str,
        device_id: &str,
        ed25519_pub: &[u8],
        token_hash: &[u8],
        kind: TokenKind,
        expires_at_ms: i64,
    ) -> Result<(), AuthStoreError> {
        Self::insert_token_row(
            &self.conn,
            token_id,
            account_id,
            device_id,
            ed25519_pub,
            token_hash,
            kind,
            expires_at_ms,
        )
    }

    /// Mint and persist a new token bound to `(account_id, device_id,
    /// ed25519_pub)` with a TTL of `ttl_ms` from now. Returns the opaque bearer
    /// string (handed to the client once) and the absolute expiry (epoch ms).
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn mint(
        &mut self,
        account_id: &str,
        device_id: &str,
        ed25519_pub: &[u8],
        kind: TokenKind,
        ttl_ms: i64,
    ) -> Result<(String, i64), AuthStoreError> {
        let t = token::NewToken::generate();
        let expires_at = now_ms().saturating_add(ttl_ms);
        self.insert_token(
            &t.token_id,
            account_id,
            device_id,
            ed25519_pub,
            &t.token_hash,
            kind,
            expires_at,
        )?;
        Ok((t.bearer, expires_at))
    }

    /// Rotate a refresh token: in **one transaction**, revoke the presented
    /// refresh token (`presented_refresh_id`) and mint a fresh access + fresh
    /// refresh token bound to the same `(account_id, device_id, ed25519_pub)`.
    ///
    /// This makes every refresh secret **single-use**: a captured refresh token
    /// is bounded to one exchange, and a replay of an already-rotated refresh
    /// token fails [`Self::load_valid_token`] (it is now revoked). Doing the
    /// revoke + two inserts atomically avoids a window where the old token is
    /// revoked but the new one is not yet persisted.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn rotate_refresh(
        &mut self,
        presented_refresh_id: &str,
        account_id: &str,
        device_id: &str,
        ed25519_pub: &[u8],
        access_ttl_ms: i64,
        refresh_ttl_ms: i64,
    ) -> Result<RotatedTokens, AuthStoreError> {
        let now = now_ms();
        let access = token::NewToken::generate();
        let access_expires_at = now.saturating_add(access_ttl_ms);
        let refresh = token::NewToken::generate();
        let refresh_expires_at = now.saturating_add(refresh_ttl_ms);

        let tx = self.conn.transaction()?;
        // Single-use: revoke the presented refresh token so it cannot be reused.
        let binding = crate::auth::binding::binding_for(&tx, account_id)?;
        let consumed = tx.execute(
            "UPDATE tokens SET revoked_at = ?2
             WHERE token_id = ?1 AND revoked_at IS NULL AND kind = 'refresh'
                 AND expires_at > ?2 AND auth_tenant_id=?3 AND device_id=?4
                 AND ed25519_pub=?5 AND content_account_id IS ?6 AND binding_version=?7",
            params![
                presented_refresh_id,
                now,
                account_id,
                device_id,
                ed25519_pub,
                binding.as_ref().map(|b| &b.content_account_id),
                binding.as_ref().map_or(0, |b| b.binding_version)
            ],
        )?;
        if consumed != 1 {
            return Err(AuthStoreError::InvalidToken);
        }
        Self::insert_token_row(
            &tx,
            &access.token_id,
            account_id,
            device_id,
            ed25519_pub,
            &access.token_hash,
            TokenKind::Access,
            access_expires_at,
        )?;
        Self::insert_token_row(
            &tx,
            &refresh.token_id,
            account_id,
            device_id,
            ed25519_pub,
            &refresh.token_hash,
            TokenKind::Refresh,
            refresh_expires_at,
        )?;
        tx.commit()?;

        Ok(RotatedTokens {
            access_token: access.bearer,
            access_expires_at,
            refresh_token: refresh.bearer,
            refresh_expires_at,
        })
    }

    /// Load a bearer token of the expected `kind` **only if** it currently
    /// validates: it exists, is not revoked, is unexpired, and the presented
    /// secret matches the stored hash (constant-time). Returns `None` otherwise
    /// (a uniform "invalid token" outcome — no distinction between unknown,
    /// expired, revoked, or wrong-secret).
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn load_valid_token(
        &self,
        bearer: &str,
        kind: TokenKind,
    ) -> Result<Option<ValidatedToken>, AuthStoreError> {
        let Some((token_id, secret)) = token::parse_bearer(bearer) else {
            return Ok(None);
        };
        let row = self
            .conn
            .query_row(
                "SELECT auth_tenant_id, device_id, ed25519_pub, token_hash, kind, expires_at, revoked_at,
                        content_account_id, binding_version, scope
                 FROM tokens WHERE token_id = ?1",
                params![token_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, i64>(8)?,
                        r.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            account_id,
            device_id,
            ed25519_pub,
            token_hash,
            kind_s,
            expires_at,
            revoked_at,
            content_account_id,
            binding_version,
            scope,
        )) = row
        else {
            return Ok(None);
        };
        // Kind must match (an access token cannot be used where a refresh token
        // is required, and vice versa).
        if TokenKind::from_wire(&kind_s) != Some(kind) {
            return Ok(None);
        }
        // Revoked or expired tokens are rejected.
        if revoked_at.is_some() || expires_at <= now_ms() {
            return Ok(None);
        }
        // Constant-time secret check.
        if !token::constant_time_eq(&token::hash_secret(&secret), &token_hash) {
            return Ok(None);
        }
        let binding = self.binding(&account_id)?;
        if binding.as_ref().map(|b| &b.content_account_id) != content_account_id.as_ref()
            || binding.as_ref().map_or(0, |b| b.binding_version) != binding_version
        {
            return Ok(None);
        }
        let scope = match (scope.as_str(), content_account_id.is_some()) {
            ("control", false) => token::TokenScope::Control,
            ("content", true) => token::TokenScope::Content,
            _ => return Err(AuthStoreError::Db(rusqlite::Error::InvalidQuery)),
        };
        // A stored key of the wrong length means a corrupt row: reject.
        let Ok(ed25519_pub) = <[u8; token::ED25519_PUB_LEN]>::try_from(ed25519_pub) else {
            return Ok(None);
        };
        Ok(Some(ValidatedToken {
            token_id,
            auth_tenant_id: account_id,
            device_id,
            ed25519_pub,
            content_account_id,
            binding_version,
            scope,
        }))
    }

    /// Validate a presented **access** bearer token and resolve the
    /// authenticated principal. This is the reusable primitive WP-3c ([#197])
    /// consumes to gate the blind content routes.
    ///
    /// [#197]: https://github.com/kafkade/pergamon/issues/197
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn validate_token(&self, bearer: &str) -> Result<Option<AuthAccount>, AuthStoreError> {
        Ok(self
            .load_valid_token(bearer, TokenKind::Access)?
            .and_then(|t| {
                Some(AuthAccount {
                    account_id: t.content_account_id?,
                    auth_tenant_id: t.auth_tenant_id,
                    device_id: t.device_id,
                    binding_version: t.binding_version,
                    token_id: t.token_id,
                })
            }))
    }

    /// Revoke **all** tokens (access and refresh) for one device within an
    /// account by stamping `revoked_at`. Returns the number of rows revoked.
    /// Idempotent: already-revoked rows are left untouched.
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn revoke_device(
        &mut self,
        account_id: &str,
        device_id: &str,
    ) -> Result<usize, AuthStoreError> {
        let n = self.conn.execute(
            "UPDATE tokens SET revoked_at = ?3
             WHERE auth_tenant_id = ?1 AND device_id = ?2 AND revoked_at IS NULL",
            params![account_id, device_id, now_ms()],
        )?;
        Ok(n)
    }

    /// Revoke a single token by id (used for targeted revocation / tests).
    /// Returns the number of rows revoked (0 or 1).
    ///
    /// # Errors
    /// Returns [`AuthStoreError::Db`] on a database failure.
    pub fn revoke_token(&mut self, token_id: &str) -> Result<usize, AuthStoreError> {
        let n = self.conn.execute(
            "UPDATE tokens SET revoked_at = ?2 WHERE token_id = ?1 AND revoked_at IS NULL",
            params![token_id, now_ms()],
        )?;
        Ok(n)
    }

    /// Persistent nonsecret installation identity used in binding statements.
    ///
    /// # Errors
    /// Returns a database error if installation metadata is missing or corrupt.
    pub fn server_instance_id(&self) -> Result<String, AuthStoreError> {
        Ok(self.conn.query_row(
            "SELECT value FROM auth_metadata WHERE name='server_instance_id'",
            [],
            |r| r.get(0),
        )?)
    }

    pub(crate) const fn connection(&self) -> &Connection {
        &self.conn
    }

    pub(crate) const fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Read the tenant's one live content binding.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn binding(
        &self,
        tenant: &str,
    ) -> Result<Option<crate::auth::binding::ContentBinding>, AuthStoreError> {
        Ok(crate::auth::binding::binding_for(&self.conn, tenant)?)
    }

    /// Register a v2 identity without allocating any content namespace.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn register_unbound(
        &mut self,
        identity: &str,
        record: &[u8],
        key_id: &str,
    ) -> Result<String, AuthStoreError> {
        if self.account_exists(identity)? {
            return Err(AuthStoreError::HandleExists);
        }
        let tenant = Uuid::new_v4().simple().to_string();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO accounts (identity_handle, opaque_record, oprf_key_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![identity, record, key_id, now_ms()],
        )?;
        tx.execute(
            "INSERT INTO account_map (identity_handle, auth_tenant_id) VALUES (?1, ?2)",
            params![identity, tenant],
        )?;
        tx.commit()?;
        Ok(tenant)
    }

    /// Issue the access/refresh pair in one transaction.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn mint_pair(
        &mut self,
        tenant: &str,
        device: &str,
        public_key: &[u8; 32],
        config: token::TokenConfig,
    ) -> Result<RotatedTokens, AuthStoreError> {
        let access = token::NewToken::generate();
        let refresh = token::NewToken::generate();
        let access_expires_at = now_ms().saturating_add(config.access_ttl_ms);
        let refresh_expires_at = now_ms().saturating_add(config.refresh_ttl_ms);
        let tx = self.conn.transaction()?;
        Self::insert_token_row(
            &tx,
            &access.token_id,
            tenant,
            device,
            public_key,
            &access.token_hash,
            TokenKind::Access,
            access_expires_at,
        )?;
        Self::insert_token_row(
            &tx,
            &refresh.token_id,
            tenant,
            device,
            public_key,
            &refresh.token_hash,
            TokenKind::Refresh,
            refresh_expires_at,
        )?;
        tx.commit()?;
        Ok(RotatedTokens {
            access_token: access.bearer,
            access_expires_at,
            refresh_token: refresh.bearer,
            refresh_expires_at,
        })
    }

    /// Recheck an admitted content principal while its namespace lease is held.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn principal_is_current(&self, principal: &AuthAccount) -> Result<bool, AuthStoreError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM tokens t JOIN content_bindings b
                ON b.auth_tenant_id = t.auth_tenant_id AND b.state != 'retired'
                WHERE t.token_id = ?1 AND t.auth_tenant_id = ?2
                AND t.content_account_id = ?3 AND t.binding_version = ?4
                AND b.content_account_id = t.content_account_id
                AND b.binding_version = t.binding_version
                AND t.revoked_at IS NULL AND t.expires_at > ?5 AND t.kind = 'access'
                AND t.scope='content')",
            params![
                principal.token_id,
                principal.auth_tenant_id,
                principal.account_id,
                principal.binding_version,
                now_ms()
            ],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn old_schema_migration_preserves_issued_token_authority() {
        let path =
            std::env::temp_dir().join(format!("pergamon-auth-migration-{}.db", Uuid::new_v4()));
        let access = token::NewToken::generate();
        let public = [7_u8; 32];
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE accounts(identity_handle TEXT PRIMARY KEY,opaque_record BLOB,
                     oprf_key_id TEXT,created_at INTEGER);
                 CREATE TABLE account_map(identity_handle TEXT PRIMARY KEY,account_id TEXT UNIQUE);
                 CREATE TABLE tokens(token_id TEXT PRIMARY KEY,account_id TEXT,device_id TEXT,
                     ed25519_pub BLOB,token_hash BLOB,kind TEXT,created_at INTEGER,
                     expires_at INTEGER,revoked_at INTEGER);
                 INSERT INTO accounts VALUES('old-owner',x'00','old-oprf',0);
                 INSERT INTO account_map VALUES('old-owner','legacy-id');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tokens VALUES(?1,'legacy-id','old-device',?2,?3,'access',0,?4,NULL)",
                params![
                    access.token_id,
                    public.as_slice(),
                    access.token_hash.as_slice(),
                    now_ms() + 60_000
                ],
            )
            .unwrap();
        }
        {
            let migrated = AuthStore::open(&path).unwrap();
            let who = migrated.validate_token(&access.bearer).unwrap().unwrap();
            assert_eq!(who.account_id, "legacy-id");
            assert_eq!(who.auth_tenant_id, "legacy-id");
            assert_eq!(who.binding_version, 0);
            assert_eq!(
                migrated.account_id("old-owner").unwrap().as_deref(),
                Some("legacy-id")
            );
        }
        let reopened = AuthStore::open(&path).unwrap();
        assert!(reopened.validate_token(&access.bearer).unwrap().is_some());
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }

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

    /// Insert a token bound to a dummy key and return `(bearer, token_id)`.
    fn mint(
        store: &mut AuthStore,
        account_id: &str,
        device_id: &str,
        kind: TokenKind,
        expires_at_ms: i64,
    ) -> (String, String) {
        store.connection().execute(
            "INSERT OR IGNORE INTO accounts(identity_handle,opaque_record,oprf_key_id,created_at)
             VALUES (?1,x'00','fixture',0)",params![account_id]).unwrap();
        store
            .connection()
            .execute(
                "INSERT OR IGNORE INTO account_map(identity_handle,auth_tenant_id) VALUES (?1,?1)",
                params![account_id],
            )
            .unwrap();
        store
            .connection()
            .execute(
                "INSERT OR IGNORE INTO content_bindings
             (content_account_id, auth_tenant_id, state, binding_version)
             VALUES (?1, ?1, 'legacy_reserved', 0)",
                params![account_id],
            )
            .unwrap();
        let t = token::NewToken::generate();
        let ed25519_pub = [7u8; token::ED25519_PUB_LEN];
        store
            .insert_token(
                &t.token_id,
                account_id,
                device_id,
                &ed25519_pub,
                &t.token_hash,
                kind,
                expires_at_ms,
            )
            .unwrap();
        (t.bearer, t.token_id)
    }

    fn hour_from_now() -> i64 {
        now_ms() + 60 * 60 * 1000
    }

    #[test]
    fn access_token_validates_and_resolves_account() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let (bearer, _id) = mint(
            &mut store,
            "acct-1",
            "dev-a",
            TokenKind::Access,
            hour_from_now(),
        );
        let who = store.validate_token(&bearer).unwrap().unwrap();
        assert_eq!(who.account_id, "acct-1");
        assert_eq!(who.device_id, "dev-a");
    }

    #[test]
    fn tampered_or_unknown_bearer_is_rejected() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let (bearer, _id) = mint(
            &mut store,
            "acct-1",
            "dev-a",
            TokenKind::Access,
            hour_from_now(),
        );
        // Flip the secret: the hash no longer matches.
        let (id, _secret) = bearer.split_once('.').unwrap();
        let forged = format!("{id}.{}", "A".repeat(43));
        assert!(store.validate_token(&forged).unwrap().is_none());
        // Entirely unknown token id.
        assert!(store.validate_token("nope.nope").unwrap().is_none());
    }

    #[test]
    fn expired_access_token_is_rejected() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let past = now_ms() - 1;
        let (bearer, _id) = mint(&mut store, "acct-1", "dev-a", TokenKind::Access, past);
        assert!(store.validate_token(&bearer).unwrap().is_none());
    }

    #[test]
    fn access_and_refresh_kinds_do_not_cross() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let (access, _) = mint(
            &mut store,
            "acct-1",
            "dev-a",
            TokenKind::Access,
            hour_from_now(),
        );
        let (refresh, _) = mint(
            &mut store,
            "acct-1",
            "dev-a",
            TokenKind::Refresh,
            hour_from_now(),
        );
        // An access bearer must not validate as a refresh token, and vice versa.
        assert!(
            store
                .load_valid_token(&access, TokenKind::Refresh)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .load_valid_token(&refresh, TokenKind::Access)
                .unwrap()
                .is_none()
        );
        assert!(store.validate_token(&access).unwrap().is_some());
        assert!(
            store
                .load_valid_token(&refresh, TokenKind::Refresh)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn revoke_device_rejects_all_its_tokens() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let (access, _) = mint(
            &mut store,
            "acct-1",
            "dev-a",
            TokenKind::Access,
            hour_from_now(),
        );
        let (refresh, _) = mint(
            &mut store,
            "acct-1",
            "dev-a",
            TokenKind::Refresh,
            hour_from_now(),
        );
        // A token for a different device must survive the revocation.
        let (other, _) = mint(
            &mut store,
            "acct-1",
            "dev-b",
            TokenKind::Access,
            hour_from_now(),
        );

        assert!(store.validate_token(&access).unwrap().is_some());
        let revoked = store.revoke_device("acct-1", "dev-a").unwrap();
        assert_eq!(revoked, 2, "both dev-a tokens should be revoked");

        assert!(store.validate_token(&access).unwrap().is_none());
        assert!(
            store
                .load_valid_token(&refresh, TokenKind::Refresh)
                .unwrap()
                .is_none()
        );
        assert!(
            store.validate_token(&other).unwrap().is_some(),
            "dev-b unaffected"
        );

        // Revocation is idempotent: a second call revokes nothing new.
        assert_eq!(store.revoke_device("acct-1", "dev-a").unwrap(), 0);
    }

    #[test]
    fn tokens_are_scoped_to_one_account() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let (a_bearer, _) = mint(
            &mut store,
            "acct-A",
            "dev-a",
            TokenKind::Access,
            hour_from_now(),
        );
        let (b_bearer, _) = mint(
            &mut store,
            "acct-B",
            "dev-a",
            TokenKind::Access,
            hour_from_now(),
        );
        assert_eq!(
            store.validate_token(&a_bearer).unwrap().unwrap().account_id,
            "acct-A"
        );
        assert_eq!(
            store.validate_token(&b_bearer).unwrap().unwrap().account_id,
            "acct-B"
        );
    }

    #[test]
    fn rotate_refresh_revokes_old_and_issues_fresh_pair() {
        let mut store = AuthStore::open_in_memory().unwrap();
        let ed25519_pub = [7u8; token::ED25519_PUB_LEN];
        // Seed an initial refresh token bound to (acct-1, dev-a, key).
        let (old_refresh, old_id) = mint(
            &mut store,
            "acct-1",
            "dev-a",
            TokenKind::Refresh,
            hour_from_now(),
        );

        let rotated = store
            .rotate_refresh(
                &old_id,
                "acct-1",
                "dev-a",
                &ed25519_pub,
                60 * 60 * 1000,
                30 * 24 * 60 * 60 * 1000,
            )
            .unwrap();

        // The presented refresh token is now revoked (single-use).
        assert!(
            store
                .load_valid_token(&old_refresh, TokenKind::Refresh)
                .unwrap()
                .is_none(),
            "old refresh token is revoked after rotation"
        );
        // The fresh access token validates and is scoped to the same account/device.
        let who = store
            .validate_token(&rotated.access_token)
            .unwrap()
            .unwrap();
        assert_eq!(who.account_id, "acct-1");
        assert_eq!(who.device_id, "dev-a");
        // The fresh refresh token is usable and differs from the old one.
        assert_ne!(rotated.refresh_token, old_refresh);
        assert!(
            store
                .load_valid_token(&rotated.refresh_token, TokenKind::Refresh)
                .unwrap()
                .is_some(),
            "the rotated refresh token is valid"
        );
    }
}
