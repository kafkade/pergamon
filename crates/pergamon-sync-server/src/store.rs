// SPDX-License-Identifier: AGPL-3.0-only

//! `SQLite` persistence for the sync server (ADR-022).
//!
//! The server keeps exactly two per-account stores and understands the
//! structure of neither payload:
//!
//! - an **append-only event log**, server-sequenced by a strictly monotonic
//!   per-account `server_seq`, holding opaque encrypted event bodies, and
//! - a **content-addressed blob store**, keyed by the ciphertext hash
//!   (`ct_hash`), holding opaque encrypted bytes.
//!
//! Everything stored here is ciphertext or a server-visible header field. The
//! store never sees plaintext, entity ids, or entity types.

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::quota::{QuotaConfig, QuotaLimit};

/// Errors returned by the [`SyncStore`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A write was refused because it would exceed the account's configured
    /// storage quota (WP-3d, #198). Carries which limit was hit so the HTTP
    /// layer can render a `507` naming the dimension.
    #[error("account storage quota exceeded ({limit})")]
    QuotaExceeded {
        /// Which configured cap (bytes or object count) the write would exceed.
        limit: QuotaLimit,
    },

    /// A blob's supplied `ct_hash` did not match the SHA-256 of its bytes.
    #[error("blob hash mismatch: expected {expected}, computed {actual}")]
    BlobHashMismatch {
        /// The `ct_hash` the client claimed.
        expected: String,
        /// The hash the server actually computed over the bytes.
        actual: String,
    },

    /// An event referenced a blob that has not been uploaded (upload-before-commit).
    #[error("event references missing blob {ct_hash}; upload it before pushing")]
    MissingBlob {
        /// The absent blob's ciphertext hash.
        ct_hash: String,
    },

    /// An underlying database error.
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    /// A JSON (de)serialization error for the stored `blob_refs` list.
    #[error("blob_refs encoding error: {0}")]
    Json(#[from] serde_json::Error),
}

/// One event envelope to append, with the ciphertext body already base64-decoded.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// Wire protocol major version.
    pub protocol_version: u32,
    /// Opaque account handle.
    pub account_id: String,
    /// Opaque origin-device handle.
    pub device_id: String,
    /// Client idempotency key.
    pub change_id: String,
    /// Blinded per-entity grouping token, if any.
    pub entity_ref: Option<String>,
    /// Account key epoch that encrypted the body.
    pub key_epoch: u32,
    /// Ciphertext hashes of blobs this event depends on.
    pub blob_refs: Vec<String>,
    /// Opaque AEAD ciphertext body.
    pub ciphertext: Vec<u8>,
    /// Opaque Ed25519 event signature bytes (ADR-030). Stored and echoed
    /// verbatim; never inspected by the server.
    pub signature: Vec<u8>,
}

/// A stored event as read back on pull, with the raw ciphertext body.
#[derive(Debug, Clone)]
pub struct StoredEventRecord {
    /// Wire protocol major version recorded at append.
    pub protocol_version: u32,
    /// Opaque account handle.
    pub account_id: String,
    /// Opaque origin-device handle.
    pub device_id: String,
    /// Client idempotency key.
    pub change_id: String,
    /// Blinded per-entity grouping token, if any.
    pub entity_ref: Option<String>,
    /// Account key epoch that encrypted the body.
    pub key_epoch: u32,
    /// Ciphertext hashes of referenced blobs.
    pub blob_refs: Vec<String>,
    /// Ciphertext body size in bytes.
    pub payload_bytes: u64,
    /// Server-assigned monotonic sequence.
    pub server_seq: u64,
    /// Server receive time (epoch millis).
    pub server_committed_at: i64,
    /// Opaque AEAD ciphertext body.
    pub ciphertext: Vec<u8>,
    /// Opaque Ed25519 event signature bytes (ADR-030), echoed verbatim.
    pub signature: Vec<u8>,
}

/// The per-event result of a push.
#[derive(Debug, Clone)]
pub struct PushResultRecord {
    /// The client idempotency key this result is for.
    pub change_id: String,
    /// The assigned (or pre-existing) sequence.
    pub server_seq: u64,
    /// Whether the event already existed and was not appended again.
    pub deduplicated: bool,
}

/// The outcome of a push batch.
#[derive(Debug, Clone)]
pub struct PushOutcome {
    /// One result per submitted event, in request order.
    pub results: Vec<PushResultRecord>,
    /// The account's high-water sequence after the batch.
    pub high_water_seq: u64,
}

/// A relayed device-roster entry: an opaque signed device record plus its
/// handle. The server never interprets `bytes`.
#[derive(Debug, Clone)]
pub struct DeviceRecordRow {
    /// Opaque origin-device handle.
    pub device_id: String,
    /// Opaque signed device-record bytes.
    pub bytes: Vec<u8>,
}

/// A relayed key-wrap bundle addressed to a recipient device, with its
/// per-recipient sequence. The server never interprets `bytes`.
#[derive(Debug, Clone)]
pub struct WrappedBundleRow {
    /// Per-recipient monotonic sequence assigned on append (the cursor domain).
    pub seq: u64,
    /// Opaque sealed bundle bytes.
    pub bytes: Vec<u8>,
}

/// A relayed signed attestation with its per-account sequence. The server never
/// interprets `bytes`.
#[derive(Debug, Clone)]
pub struct AttestationRow {
    /// Per-account monotonic sequence assigned on append (the cursor domain).
    pub seq: u64,
    /// Opaque signed attestation bytes.
    pub bytes: Vec<u8>,
}

/// The result of appending an opaque, sequence-assigned relay artifact.
#[derive(Debug, Clone)]
pub struct AppendResult {
    /// The assigned (or pre-existing, on dedup) sequence.
    pub seq: u64,
    /// `true` when identical bytes already existed and were not appended again.
    pub deduplicated: bool,
}

/// A snapshot of an account's metered ciphertext usage (WP-3d, #198).
///
/// Measured on **ciphertext size and counts only** — the opaque blob bytes
/// (`blobs.byte_len`) and event payload bytes (`events.payload_bytes`). This is
/// metadata the operator necessarily learns by storing the bytes; computing it
/// never inspects or decodes ciphertext (design §2.5, content-blindness).
///
/// The onboarding-artifact relay tables (device records, wrapped bundles,
/// attestations, recovery blobs) are intentionally **not** metered: they are
/// bounded, operationally negligible, and out of scope for storage quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageRecord {
    /// Total bytes of opaque blob ciphertext stored for the account.
    pub blob_bytes: u64,
    /// Number of distinct blobs stored for the account.
    pub blob_count: u64,
    /// Total bytes of event ciphertext (`payload_bytes`) stored for the account.
    pub event_bytes: u64,
    /// Number of events stored for the account.
    pub event_count: u64,
}

impl UsageRecord {
    /// Combined metered ciphertext bytes (blobs + events).
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.blob_bytes.saturating_add(self.event_bytes)
    }

    /// Combined metered object count (blobs + events).
    #[must_use]
    pub const fn total_objects(&self) -> u64 {
        self.blob_count.saturating_add(self.event_count)
    }
}

/// Compute the content address (`ct_hash`) of some bytes: lowercase-hex SHA-256.
///
/// Blobs are addressed by the hash of their *ciphertext*, so this is the same
/// function a client uses before uploading.
#[must_use]
pub fn ct_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a String is infallible.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Current wall-clock time in epoch milliseconds.
fn now_millis() -> i64 {
    let now = OffsetDateTime::now_utc();
    now.unix_timestamp() * 1000 + i64::from(now.millisecond())
}

/// `SQLite`-backed persistence for the encrypted event log and blob store.
pub struct SyncStore {
    conn: Connection,
    /// Per-tenant storage quota policy (WP-3d, #198). Defaults to
    /// [`QuotaConfig::unlimited`] so blind / existing deployments are unchanged;
    /// set an explicit cap with [`SyncStore::with_quota`].
    quota: QuotaConfig,
}

impl SyncStore {
    /// Open (creating if needed) a file-backed store and initialize its schema.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the database cannot be opened or migrated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        let store = Self {
            conn,
            quota: QuotaConfig::default(),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Open an in-memory store (used by tests).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the schema cannot be created.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn,
            quota: QuotaConfig::default(),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Apply a per-tenant storage [`QuotaConfig`] to this store (WP-3d, #198).
    ///
    /// A builder-style setter so existing call sites (`SyncStore::open(..)?`)
    /// keep the unlimited default and stay behavior-identical; the server binary
    /// calls this once at startup when the operator configures a cap.
    #[must_use]
    pub const fn with_quota(mut self, quota: QuotaConfig) -> Self {
        self.quota = quota;
        self
    }

    /// The store's configured per-tenant storage quota.
    #[must_use]
    pub const fn quota(&self) -> QuotaConfig {
        self.quota
    }

    /// Create the two per-account stores if they do not yet exist.
    fn init_schema(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                account_id          TEXT    NOT NULL,
                server_seq          INTEGER NOT NULL,
                change_id           TEXT    NOT NULL,
                device_id           TEXT    NOT NULL,
                protocol_version    INTEGER NOT NULL,
                entity_ref          TEXT,
                key_epoch           INTEGER NOT NULL,
                blob_refs           TEXT    NOT NULL,
                payload_bytes       INTEGER NOT NULL,
                ciphertext          BLOB    NOT NULL,
                signature           BLOB    NOT NULL DEFAULT (x''),
                server_committed_at INTEGER NOT NULL,
                PRIMARY KEY (account_id, server_seq)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_events_change_id
                ON events (account_id, change_id);

            -- WP-3d (#198) covering indexes for the per-tenant usage aggregates.
            -- `events.ciphertext` / `blobs.bytes` are large inline BLOBs, so a
            -- bare `SUM(payload_bytes)/COUNT(*) WHERE account_id=?` would scan the
            -- full rows (pulling ciphertext + overflow pages off disk) just to
            -- total an integer — and that runs on every quota-checked write and
            -- every /v1/usage read. These `(account_id, <size>)` indexes let SQLite
            -- answer the aggregate index-only (covering), never touching the
            -- ciphertext, which is what makes the A1 compute-on-the-fly
            -- accounting cheap enough to defer materialized counters (A2).
            -- `IF NOT EXISTS` makes this a safe no-op on an existing database.
            CREATE INDEX IF NOT EXISTS idx_events_usage
                ON events (account_id, payload_bytes);

            CREATE TABLE IF NOT EXISTS blobs (
                account_id  TEXT    NOT NULL,
                ct_hash     TEXT    NOT NULL,
                bytes       BLOB    NOT NULL,
                byte_len    INTEGER NOT NULL,
                created_at  INTEGER NOT NULL,
                PRIMARY KEY (account_id, ct_hash)
            );
            CREATE INDEX IF NOT EXISTS idx_blobs_usage
                ON blobs (account_id, byte_len);

            -- Opaque onboarding-artifact relay stores (ADR-024, #125). The
            -- server stores and serves these bytes verbatim and never decodes
            -- them; they are separate from the ADR-022 event frame above.

            -- Self-signed device roster entries, one row per device, replaceable
            -- when a device re-publishes its (stable) record.
            CREATE TABLE IF NOT EXISTS device_records (
                account_id  TEXT    NOT NULL,
                device_id   TEXT    NOT NULL,
                bytes       BLOB    NOT NULL,
                updated_at  INTEGER NOT NULL,
                PRIMARY KEY (account_id, device_id)
            );

            -- Sealed key-wrap bundles (enrollment + rotation re-wraps) addressed
            -- to a recipient device. Append-only, per-recipient monotonic seq,
            -- content-hash deduplicated so retries are idempotent.
            CREATE TABLE IF NOT EXISTS wrapped_bundles (
                account_id          TEXT    NOT NULL,
                recipient_device_id TEXT    NOT NULL,
                seq                 INTEGER NOT NULL,
                content_hash        TEXT    NOT NULL,
                bytes               BLOB    NOT NULL,
                created_at          INTEGER NOT NULL,
                PRIMARY KEY (account_id, recipient_device_id, seq)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_wrapped_content
                ON wrapped_bundles (account_id, recipient_device_id, content_hash);

            -- Signed trust/revocation attestations, visible to every device.
            -- Append-only, per-account monotonic seq, content-hash deduplicated.
            CREATE TABLE IF NOT EXISTS attestations (
                account_id   TEXT    NOT NULL,
                seq          INTEGER NOT NULL,
                content_hash TEXT    NOT NULL,
                bytes        BLOB    NOT NULL,
                created_at   INTEGER NOT NULL,
                PRIMARY KEY (account_id, seq)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_attestation_content
                ON attestations (account_id, content_hash);

            -- Optional single Argon2id-wrapped recovery blob per account.
            CREATE TABLE IF NOT EXISTS recovery_blobs (
                account_id  TEXT    NOT NULL,
                bytes       BLOB    NOT NULL,
                updated_at  INTEGER NOT NULL,
                PRIMARY KEY (account_id)
            );",
        )?;
        Ok(())
    }

    /// Return `true` if the account already holds a blob with this `ct_hash`.
    fn blob_present(
        conn: &Connection,
        account_id: &str,
        ct_hash: &str,
    ) -> Result<bool, StoreError> {
        let found = conn
            .query_row(
                "SELECT 1 FROM blobs WHERE account_id = ?1 AND ct_hash = ?2",
                params![account_id, ct_hash],
                |_| Ok(()),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Compute an account's metered ciphertext usage from the live content tables
    /// (WP-3d, #198).
    ///
    /// This is the **A1 "compute on-the-fly"** accounting strategy: usage is
    /// summed directly from `blobs.byte_len` + `events.payload_bytes` on every
    /// read/enforcement, so it is always exactly the true stored size and can
    /// never drift from the content. Because it reads live rows it is inherently
    /// dedup-correct (a re-uploaded blob or a deduped event contributes nothing)
    /// and — a documented seam — will **auto-correct on deletion** once a delete
    /// path exists (WP-10 GDPR deletion), needing no separate counter decrement.
    ///
    /// Takes a `&Connection` so it can run either standalone or inside the
    /// `push_events` write transaction (a `&Transaction` derefs to `&Connection`).
    fn account_usage_conn(conn: &Connection, account_id: &str) -> Result<UsageRecord, StoreError> {
        let (blob_bytes, blob_count): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(byte_len), 0), COUNT(*) FROM blobs WHERE account_id = ?1",
            params![account_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (event_bytes, event_count): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(payload_bytes), 0), COUNT(*) FROM events WHERE account_id = ?1",
            params![account_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(UsageRecord {
            blob_bytes: u64::try_from(blob_bytes).unwrap_or(0),
            blob_count: u64::try_from(blob_count).unwrap_or(0),
            event_bytes: u64::try_from(event_bytes).unwrap_or(0),
            event_count: u64::try_from(event_count).unwrap_or(0),
        })
    }

    /// An account's current metered ciphertext usage (blobs + events).
    ///
    /// Backs the `GET /v1/usage/{account_id}` billing/metrics endpoint. See
    /// [`Self::account_usage_conn`] for the accounting strategy and its
    /// content-blindness guarantee.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn account_usage(&self, account_id: &str) -> Result<UsageRecord, StoreError> {
        Self::account_usage_conn(&self.conn, account_id)
    }

    /// Store an opaque blob, addressed by the SHA-256 of its (ciphertext) bytes.
    ///
    /// Idempotent: re-uploading an existing `ct_hash` is a no-op because the
    /// address *is* the content. Verifies the supplied `ct_hash` matches the
    /// bytes so the log can never gain a mislabeled reference.
    ///
    /// **Quota (WP-3d, #198):** when a non-unlimited [`QuotaConfig`] is set, a
    /// blob that is *new* to the account (not already present) is refused with
    /// [`StoreError::QuotaExceeded`] if storing it would push the account's
    /// combined ciphertext usage past a configured cap. An idempotent re-upload
    /// of an already-present blob adds nothing and is always allowed, even while
    /// the account is at/over quota. With the default unlimited quota this method
    /// is byte-for-byte the original fast path.
    ///
    /// # Errors
    /// Returns [`StoreError::BlobHashMismatch`] if `ct_hash` does not match the
    /// bytes, [`StoreError::QuotaExceeded`] if a configured quota would be
    /// exceeded, or [`StoreError::Db`] on a database failure.
    pub fn blob_put(
        &self,
        account_id: &str,
        ct_hash: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let actual = crate::store::ct_hash(bytes);
        if actual != ct_hash {
            return Err(StoreError::BlobHashMismatch {
                expected: ct_hash.to_owned(),
                actual,
            });
        }

        // Quota enforcement runs only when a cap is configured. The unlimited
        // default takes the original fast path unchanged (no extra queries).
        if !self.quota.is_unlimited() {
            // A re-upload of an already-present blob adds 0 bytes/objects, so it
            // is exempt from the cap (idempotent no-op writes stay allowed).
            if !Self::blob_present(&self.conn, account_id, ct_hash)? {
                let usage = Self::account_usage_conn(&self.conn, account_id)?;
                let new_bytes = usage
                    .total_bytes()
                    .saturating_add(bytes.len().try_into().unwrap_or(u64::MAX));
                let new_objects = usage.total_objects().saturating_add(1);
                self.quota
                    .check(new_bytes, new_objects)
                    .map_err(|limit| StoreError::QuotaExceeded { limit })?;
            }
        }

        let byte_len = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT OR IGNORE INTO blobs (account_id, ct_hash, bytes, byte_len, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account_id, ct_hash, bytes, byte_len, now_millis()],
        )?;
        Ok(())
    }

    /// Fetch an opaque blob's bytes, or `None` if the account has no such blob.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn blob_get(&self, account_id: &str, ct_hash: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let bytes = self
            .conn
            .query_row(
                "SELECT bytes FROM blobs WHERE account_id = ?1 AND ct_hash = ?2",
                params![account_id, ct_hash],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        Ok(bytes)
    }

    /// Partition a set of hashes into those present and those missing for an
    /// account (the dedup probe that precedes a push).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn blob_probe(
        &self,
        account_id: &str,
        hashes: &[String],
    ) -> Result<(Vec<String>, Vec<String>), StoreError> {
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for hash in hashes {
            if Self::blob_present(&self.conn, account_id, hash)? {
                present.push(hash.clone());
            } else {
                missing.push(hash.clone());
            }
        }
        Ok((present, missing))
    }

    /// Append a batch of events to an account's log.
    ///
    /// The whole batch commits atomically. Within it, each event is either:
    /// - **deduplicated** — a matching `change_id` already exists, so its
    ///   existing `server_seq` is returned and nothing is appended; or
    /// - **appended** — after verifying every referenced blob is already
    ///   present (upload-before-commit), a fresh monotonic `server_seq` is
    ///   assigned.
    ///
    /// This makes push idempotent under retry: resending an accepted batch
    /// dedupes rather than duplicating.
    ///
    /// **Quota (WP-3d, #198):** when a non-unlimited [`QuotaConfig`] is set, the
    /// batch's newly-appended events are metered inside this transaction; if they
    /// would push the account past a configured cap the call returns
    /// [`StoreError::QuotaExceeded`] and the **whole batch rolls back** (no
    /// partial commit). Deduped events add nothing.
    ///
    /// # Errors
    /// Returns [`StoreError::MissingBlob`] if any new event references an
    /// un-uploaded blob (the batch is rolled back), [`StoreError::QuotaExceeded`]
    /// if a configured quota would be exceeded (the batch is rolled back),
    /// [`StoreError::Json`] on a `blob_refs` encoding failure, or
    /// [`StoreError::Db`] on a database error.
    pub fn push_events(
        &mut self,
        account_id: &str,
        events: &[EventRecord],
    ) -> Result<PushOutcome, StoreError> {
        // Copy the (Copy) quota policy out before borrowing `self.conn` for the
        // transaction, so enforcement can read it without a borrow conflict.
        let quota = self.quota;
        let tx = self.conn.transaction()?;
        let committed_at = now_millis();

        let max_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(server_seq), 0) FROM events WHERE account_id = ?1",
            params![account_id],
            |row| row.get(0),
        )?;
        let mut next_seq = u64::try_from(max_seq).unwrap_or(0);

        // WP-3d (#198): when a cap is configured, meter this batch inside the same
        // write transaction so an over-quota batch rolls back atomically — a
        // partial batch never commits. Deduped events are not re-inserted and so
        // add nothing, making enforcement inherently dedup-correct. The unlimited
        // default skips all of this and is byte-for-byte the original path.
        let enforce_quota = !quota.is_unlimited();
        let (mut projected_bytes, mut projected_objects) = if enforce_quota {
            let usage = Self::account_usage_conn(&tx, account_id)?;
            (usage.total_bytes(), usage.total_objects())
        } else {
            (0, 0)
        };

        let mut results = Vec::with_capacity(events.len());
        for ev in events {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT server_seq FROM events WHERE account_id = ?1 AND change_id = ?2",
                    params![account_id, ev.change_id],
                    |row| row.get(0),
                )
                .optional()?;

            if let Some(seq) = existing {
                results.push(PushResultRecord {
                    change_id: ev.change_id.clone(),
                    server_seq: u64::try_from(seq).unwrap_or(0),
                    deduplicated: true,
                });
                continue;
            }

            for hash in &ev.blob_refs {
                if !Self::blob_present(&tx, account_id, hash)? {
                    return Err(StoreError::MissingBlob {
                        ct_hash: hash.clone(),
                    });
                }
            }

            // Meter this newly-appended event against the account's cap before
            // inserting it. Returning here drops `tx` and rolls the batch back.
            if enforce_quota {
                let payload_len = u64::try_from(ev.ciphertext.len()).unwrap_or(u64::MAX);
                projected_bytes = projected_bytes.saturating_add(payload_len);
                projected_objects = projected_objects.saturating_add(1);
                quota
                    .check(projected_bytes, projected_objects)
                    .map_err(|limit| StoreError::QuotaExceeded { limit })?;
            }

            next_seq += 1;
            let seq_i = i64::try_from(next_seq).unwrap_or(i64::MAX);
            let blob_refs_json = serde_json::to_string(&ev.blob_refs)?;
            let payload_bytes = i64::try_from(ev.ciphertext.len()).unwrap_or(i64::MAX);

            tx.execute(
                "INSERT INTO events (
                    account_id, server_seq, change_id, device_id, protocol_version,
                    entity_ref, key_epoch, blob_refs, payload_bytes, ciphertext,
                    signature, server_committed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    account_id,
                    seq_i,
                    ev.change_id,
                    ev.device_id,
                    ev.protocol_version,
                    ev.entity_ref,
                    ev.key_epoch,
                    blob_refs_json,
                    payload_bytes,
                    ev.ciphertext,
                    ev.signature,
                    committed_at,
                ],
            )?;

            results.push(PushResultRecord {
                change_id: ev.change_id.clone(),
                server_seq: next_seq,
                deduplicated: false,
            });
        }

        tx.commit()?;
        Ok(PushOutcome {
            results,
            high_water_seq: next_seq,
        })
    }

    /// Read a page of events with `server_seq > after`, ascending, up to `limit`.
    ///
    /// # Errors
    /// Returns [`StoreError::Json`] if a stored `blob_refs` value cannot be
    /// decoded, or [`StoreError::Db`] on a database error.
    pub fn pull_events(
        &self,
        account_id: &str,
        after: u64,
        limit: u32,
    ) -> Result<Vec<StoredEventRecord>, StoreError> {
        let after_i = i64::try_from(after).unwrap_or(i64::MAX);
        let limit_i = i64::from(limit);

        let mut stmt = self.conn.prepare(
            "SELECT protocol_version, account_id, device_id, change_id, entity_ref,
                    key_epoch, blob_refs, payload_bytes, server_seq, server_committed_at,
                    ciphertext, signature
             FROM events
             WHERE account_id = ?1 AND server_seq > ?2
             ORDER BY server_seq ASC
             LIMIT ?3",
        )?;

        let raw_rows = stmt.query_map(params![account_id, after_i, limit_i], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Vec<u8>>(10)?,
                row.get::<_, Vec<u8>>(11)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in raw_rows {
            let (
                protocol_version,
                account,
                device_id,
                change_id,
                entity_ref,
                key_epoch,
                blob_refs_json,
                payload_bytes,
                server_seq,
                server_committed_at,
                ciphertext,
                signature,
            ) = row?;
            let blob_refs: Vec<String> = serde_json::from_str(&blob_refs_json)?;
            out.push(StoredEventRecord {
                protocol_version,
                account_id: account,
                device_id,
                change_id,
                entity_ref,
                key_epoch,
                blob_refs,
                payload_bytes: u64::try_from(payload_bytes).unwrap_or(0),
                server_seq: u64::try_from(server_seq).unwrap_or(0),
                server_committed_at,
                ciphertext,
                signature,
            });
        }
        Ok(out)
    }

    /// The account's current high-water `server_seq` (0 if it has no events).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn high_water(&self, account_id: &str) -> Result<u64, StoreError> {
        let max: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(server_seq), 0) FROM events WHERE account_id = ?1",
            params![account_id],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(max).unwrap_or(0))
    }

    // --- Opaque onboarding-artifact relay (ADR-024, #125) ---------------------
    //
    // All methods below store and serve opaque bytes verbatim. The server never
    // decodes, validates, or interprets them beyond content-hash deduplication;
    // authenticity is enforced entirely client-side by signatures the server
    // cannot read.

    /// Publish (or replace) a device's opaque signed record.
    ///
    /// Idempotent by `(account_id, device_id)`: a device re-publishing its
    /// record overwrites the previous bytes.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn device_record_put(
        &self,
        account_id: &str,
        device_id: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO device_records (account_id, device_id, bytes, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, device_id)
             DO UPDATE SET bytes = excluded.bytes, updated_at = excluded.updated_at",
            params![account_id, device_id, bytes, now_millis()],
        )?;
        Ok(())
    }

    /// Fetch one device's opaque record, or `None` if absent.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn device_record_get(
        &self,
        account_id: &str,
        device_id: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let bytes = self
            .conn
            .query_row(
                "SELECT bytes FROM device_records WHERE account_id = ?1 AND device_id = ?2",
                params![account_id, device_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        Ok(bytes)
    }

    /// List every device record for an account, ordered by `device_id`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn device_records_list(
        &self,
        account_id: &str,
    ) -> Result<Vec<DeviceRecordRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT device_id, bytes FROM device_records
             WHERE account_id = ?1 ORDER BY device_id ASC",
        )?;
        let rows = stmt.query_map(params![account_id], |row| {
            Ok(DeviceRecordRow {
                device_id: row.get::<_, String>(0)?,
                bytes: row.get::<_, Vec<u8>>(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Append an opaque key-wrap bundle addressed to a recipient device.
    ///
    /// Idempotent by content: re-submitting identical bytes for the same
    /// recipient returns the existing sequence instead of appending a duplicate.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn wrapped_bundle_put(
        &mut self,
        account_id: &str,
        recipient_device_id: &str,
        bytes: &[u8],
    ) -> Result<AppendResult, StoreError> {
        let hash = ct_hash(bytes);
        let tx = self.conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT seq FROM wrapped_bundles
                 WHERE account_id = ?1 AND recipient_device_id = ?2 AND content_hash = ?3",
                params![account_id, recipient_device_id, hash],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(seq) = existing {
            tx.commit()?;
            return Ok(AppendResult {
                seq: u64::try_from(seq).unwrap_or(0),
                deduplicated: true,
            });
        }
        let max_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM wrapped_bundles
             WHERE account_id = ?1 AND recipient_device_id = ?2",
            params![account_id, recipient_device_id],
            |row| row.get(0),
        )?;
        let seq = max_seq + 1;
        tx.execute(
            "INSERT INTO wrapped_bundles
                (account_id, recipient_device_id, seq, content_hash, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                account_id,
                recipient_device_id,
                seq,
                hash,
                bytes,
                now_millis()
            ],
        )?;
        tx.commit()?;
        Ok(AppendResult {
            seq: u64::try_from(seq).unwrap_or(0),
            deduplicated: false,
        })
    }

    /// List key-wrap bundles for a recipient device with `seq > after`,
    /// ascending.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn wrapped_bundles_list(
        &self,
        account_id: &str,
        recipient_device_id: &str,
        after: u64,
        limit: u32,
    ) -> Result<Vec<WrappedBundleRow>, StoreError> {
        let after_i = i64::try_from(after).unwrap_or(i64::MAX);
        let limit_i = i64::from(limit);
        let mut stmt = self.conn.prepare(
            "SELECT seq, bytes FROM wrapped_bundles
             WHERE account_id = ?1 AND recipient_device_id = ?2 AND seq > ?3
             ORDER BY seq ASC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![account_id, recipient_device_id, after_i, limit_i],
            |row| {
                Ok(WrappedBundleRow {
                    seq: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                    bytes: row.get::<_, Vec<u8>>(1)?,
                })
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Append an opaque signed attestation to an account's roster history.
    ///
    /// Idempotent by content: re-submitting identical bytes returns the existing
    /// sequence instead of appending a duplicate.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn attestation_append(
        &mut self,
        account_id: &str,
        bytes: &[u8],
    ) -> Result<AppendResult, StoreError> {
        let hash = ct_hash(bytes);
        let tx = self.conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT seq FROM attestations WHERE account_id = ?1 AND content_hash = ?2",
                params![account_id, hash],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(seq) = existing {
            tx.commit()?;
            return Ok(AppendResult {
                seq: u64::try_from(seq).unwrap_or(0),
                deduplicated: true,
            });
        }
        let max_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM attestations WHERE account_id = ?1",
            params![account_id],
            |row| row.get(0),
        )?;
        let seq = max_seq + 1;
        tx.execute(
            "INSERT INTO attestations (account_id, seq, content_hash, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account_id, seq, hash, bytes, now_millis()],
        )?;
        tx.commit()?;
        Ok(AppendResult {
            seq: u64::try_from(seq).unwrap_or(0),
            deduplicated: false,
        })
    }

    /// List attestations for an account with `seq > after`, ascending.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn attestations_list(
        &self,
        account_id: &str,
        after: u64,
        limit: u32,
    ) -> Result<Vec<AttestationRow>, StoreError> {
        let after_i = i64::try_from(after).unwrap_or(i64::MAX);
        let limit_i = i64::from(limit);
        let mut stmt = self.conn.prepare(
            "SELECT seq, bytes FROM attestations
             WHERE account_id = ?1 AND seq > ?2
             ORDER BY seq ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![account_id, after_i, limit_i], |row| {
            Ok(AttestationRow {
                seq: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                bytes: row.get::<_, Vec<u8>>(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Store (or replace) an account's single opaque recovery blob.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn recovery_blob_put(&self, account_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO recovery_blobs (account_id, bytes, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id)
             DO UPDATE SET bytes = excluded.bytes, updated_at = excluded.updated_at",
            params![account_id, bytes, now_millis()],
        )?;
        Ok(())
    }

    /// Fetch an account's opaque recovery blob, or `None` if none is enabled.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] on a database failure.
    pub fn recovery_blob_get(&self, account_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let bytes = self
            .conn
            .query_row(
                "SELECT bytes FROM recovery_blobs WHERE account_id = ?1",
                params![account_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]

    use super::*;

    const ACCT: &str = "acct-1";

    /// Build a minimal event record with the given `change_id` and ciphertext.
    fn ev(change_id: &str, ciphertext: &[u8]) -> EventRecord {
        EventRecord {
            protocol_version: 1,
            account_id: ACCT.to_string(),
            device_id: "dev-1".to_string(),
            change_id: change_id.to_string(),
            entity_ref: None,
            key_epoch: 1,
            blob_refs: Vec::new(),
            ciphertext: ciphertext.to_vec(),
            signature: Vec::new(),
        }
    }

    /// Put a blob whose `ct_hash` is derived from its bytes (the client contract).
    fn put_blob(store: &SyncStore, account: &str, bytes: &[u8]) -> Result<(), StoreError> {
        store.blob_put(account, &ct_hash(bytes), bytes)
    }

    #[test]
    fn account_usage_sums_blobs_and_events() {
        let mut store = SyncStore::open_in_memory().unwrap();
        put_blob(&store, ACCT, b"blob-aaaa").unwrap(); // 9 bytes
        put_blob(&store, ACCT, b"blob-bb").unwrap(); //   7 bytes
        store
            .push_events(ACCT, &[ev("c1", b"12345"), ev("c2", b"678")])
            .unwrap(); // 5 + 3 = 8 bytes, 2 events

        let usage = store.account_usage(ACCT).unwrap();
        assert_eq!(usage.blob_bytes, 16);
        assert_eq!(usage.blob_count, 2);
        assert_eq!(usage.event_bytes, 8);
        assert_eq!(usage.event_count, 2);
        assert_eq!(usage.total_bytes(), 24);
        assert_eq!(usage.total_objects(), 4);

        // A different account is metered independently.
        assert_eq!(
            store.account_usage("other").unwrap(),
            UsageRecord::default()
        );
    }

    #[test]
    fn reuploading_a_blob_does_not_inflate_usage() {
        let store = SyncStore::open_in_memory().unwrap();
        put_blob(&store, ACCT, b"same-bytes").unwrap();
        let first = store.account_usage(ACCT).unwrap();
        // Re-upload the identical blob twice more.
        put_blob(&store, ACCT, b"same-bytes").unwrap();
        put_blob(&store, ACCT, b"same-bytes").unwrap();
        assert_eq!(store.account_usage(ACCT).unwrap(), first);
        assert_eq!(first.blob_count, 1);
    }

    #[test]
    fn deduped_push_does_not_inflate_usage() {
        let mut store = SyncStore::open_in_memory().unwrap();
        let batch = [ev("c1", b"aaaa"), ev("c2", b"bb")];
        store.push_events(ACCT, &batch).unwrap();
        let first = store.account_usage(ACCT).unwrap();
        // Resend the exact same batch: every event dedupes on change_id.
        store.push_events(ACCT, &batch).unwrap();
        assert_eq!(store.account_usage(ACCT).unwrap(), first);
        assert_eq!(first.event_count, 2);
        assert_eq!(store.high_water(ACCT).unwrap(), 2);
    }

    #[test]
    fn blob_put_rejects_over_byte_quota_and_leaves_store_unchanged() {
        let store = SyncStore::open_in_memory()
            .unwrap()
            .with_quota(QuotaConfig {
                max_account_bytes: 10,
                max_account_objects: 0,
            });
        // A 10-byte blob is exactly at the cap: allowed.
        put_blob(&store, ACCT, b"0123456789").unwrap();
        let before = store.account_usage(ACCT).unwrap();

        // A second, distinct blob would push total bytes to 20 > 10: rejected.
        let err = put_blob(&store, ACCT, b"another").unwrap_err();
        assert!(matches!(
            err,
            StoreError::QuotaExceeded {
                limit: QuotaLimit::Bytes
            }
        ));
        // The store is unchanged by the rejected write.
        assert_eq!(store.account_usage(ACCT).unwrap(), before);

        // An idempotent re-upload of the already-present blob is still allowed
        // even though the account is at its cap.
        put_blob(&store, ACCT, b"0123456789").unwrap();
        assert_eq!(store.account_usage(ACCT).unwrap(), before);
    }

    #[test]
    fn blob_put_rejects_over_object_quota() {
        let store = SyncStore::open_in_memory()
            .unwrap()
            .with_quota(QuotaConfig {
                max_account_bytes: 0,
                max_account_objects: 1,
            });
        put_blob(&store, ACCT, b"one").unwrap();
        let err = put_blob(&store, ACCT, b"two").unwrap_err();
        assert!(matches!(
            err,
            StoreError::QuotaExceeded {
                limit: QuotaLimit::Objects
            }
        ));
        assert_eq!(store.account_usage(ACCT).unwrap().blob_count, 1);
    }

    #[test]
    fn push_events_over_quota_rolls_back_the_whole_batch() {
        let mut store = SyncStore::open_in_memory()
            .unwrap()
            .with_quota(QuotaConfig {
                max_account_bytes: 12,
                max_account_objects: 0,
            });
        // Two 10-byte events: the first fits (10 <= 12), the second would push
        // the total to 20 > 12. The whole batch must roll back atomically.
        let err = store
            .push_events(ACCT, &[ev("c1", b"0123456789"), ev("c2", b"abcdefghij")])
            .unwrap_err();
        assert!(matches!(
            err,
            StoreError::QuotaExceeded {
                limit: QuotaLimit::Bytes
            }
        ));
        // Nothing committed: no events, high-water still 0.
        assert_eq!(store.account_usage(ACCT).unwrap(), UsageRecord::default());
        assert_eq!(store.high_water(ACCT).unwrap(), 0);
    }

    #[test]
    fn unlimited_quota_allows_large_writes() {
        let mut store = SyncStore::open_in_memory().unwrap();
        assert!(store.quota().is_unlimited());
        let big = vec![0_u8; 1_000_000];
        put_blob(&store, ACCT, &big).unwrap();
        store.push_events(ACCT, &[ev("c1", &big)]).unwrap();
        let usage = store.account_usage(ACCT).unwrap();
        assert_eq!(usage.blob_bytes, 1_000_000);
        assert_eq!(usage.event_bytes, 1_000_000);
    }

    /// The usage aggregates must be answerable from the WP-3d covering indexes
    /// alone, so summing sizes never pulls the inline ciphertext BLOBs off disk.
    #[test]
    fn usage_aggregates_use_covering_indexes() {
        let store = SyncStore::open_in_memory().unwrap();
        let plan = |sql: &str| -> String {
            let mut stmt = store
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            rows.join(" | ")
        };

        let blobs =
            plan("SELECT COALESCE(SUM(byte_len), 0), COUNT(*) FROM blobs WHERE account_id = 'a'");
        assert!(
            blobs.contains("idx_blobs_usage"),
            "blobs usage aggregate should use idx_blobs_usage, plan was: {blobs}"
        );

        let events = plan(
            "SELECT COALESCE(SUM(payload_bytes), 0), COUNT(*) FROM events WHERE account_id = 'a'",
        );
        assert!(
            events.contains("idx_events_usage"),
            "events usage aggregate should use idx_events_usage, plan was: {events}"
        );
    }
}
