-- V13: client sync engine state (ADR-022 / ADR-023, issue #126)
--
-- All sync bookkeeping lives in dedicated tables so canonical entity tables are
-- untouched. Every local mutation writes its canonical row(s) *and* an outbox
-- row (+ per-field clocks) in one transaction; the engine encrypts and pushes
-- the outbox, and applies pulled changes through the ADR-023 merge policy,
-- recording per-field clocks, set-edge tombstones, delete tombstones, applied
-- change ids (idempotency), and conflict-copy losers (the conflict inbox).

-- Single-row engine state: account/device identity, pull cursor, and the last
-- local hybrid logical clock so it survives restarts.
CREATE TABLE sync_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    account_id      TEXT,
    device_id       TEXT,
    key_epoch       INTEGER NOT NULL DEFAULT 0,
    cursor_seq      INTEGER NOT NULL DEFAULT 0,
    hlc_wall_millis INTEGER NOT NULL DEFAULT 0,
    hlc_counter     INTEGER NOT NULL DEFAULT 0,
    server_url      TEXT
);

INSERT INTO sync_state (id) VALUES (1);

-- Pending local changes awaiting push. `body` is the serialized (plaintext)
-- ChangeBody; the engine encrypts it. A row is durable the moment it commits
-- and stays pending until the server acknowledges its change_id (idempotent).
CREATE TABLE sync_outbox (
    change_id   TEXT PRIMARY KEY NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    op          TEXT NOT NULL,
    body        BLOB NOT NULL,
    blob_refs   TEXT NOT NULL DEFAULT '[]',
    local_seq   INTEGER NOT NULL,
    acked_seq   INTEGER,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_sync_outbox_pending ON sync_outbox(local_seq) WHERE acked_seq IS NULL;

-- Monotonic local ordering source for outbox rows.
CREATE TABLE sync_local_seq (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    next INTEGER NOT NULL DEFAULT 1
);

INSERT INTO sync_local_seq (id) VALUES (1);

-- Per-field hybrid logical clock, for LWW ordering and concurrency detection.
CREATE TABLE sync_entity_clock (
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    field       TEXT NOT NULL,
    wall_millis INTEGER NOT NULL,
    counter     INTEGER NOT NULL,
    device_id   TEXT NOT NULL,
    PRIMARY KEY (entity_type, entity_id, field)
) WITHOUT ROWID;

-- Observed-remove state for membership edges (tag / collection membership):
-- the greatest add clock and the greatest remove clock seen for each edge.
CREATE TABLE sync_set_edge (
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    add_wall    INTEGER,
    add_counter INTEGER,
    add_device  TEXT,
    rem_wall    INTEGER,
    rem_counter INTEGER,
    rem_device  TEXT,
    PRIMARY KEY (entity_type, entity_id)
) WITHOUT ROWID;

-- Delete tombstones for non-edge entities, keyed by the winning delete clock.
CREATE TABLE sync_tombstones (
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    wall_millis INTEGER NOT NULL,
    counter     INTEGER NOT NULL,
    device_id   TEXT NOT NULL,
    PRIMARY KEY (entity_type, entity_id)
) WITHOUT ROWID;

-- Idempotency guard: change_ids already applied on pull, so re-pull/re-apply is
-- a no-op even across crashes.
CREATE TABLE sync_applied (
    change_id  TEXT PRIMARY KEY NOT NULL,
    server_seq INTEGER NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
) WITHOUT ROWID;

-- The conflict inbox: losers of a conflict-copy merge on authored prose. The
-- live value is written to the canonical entity; the loser is preserved here
-- for the user to reconcile or dismiss.
CREATE TABLE sync_conflicts (
    id            TEXT PRIMARY KEY NOT NULL,
    entity_type   TEXT NOT NULL,
    entity_id     TEXT NOT NULL,
    field         TEXT NOT NULL,
    loser_value   TEXT NOT NULL,
    loser_wall    INTEGER NOT NULL,
    loser_counter INTEGER NOT NULL,
    loser_device  TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    dismissed     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_sync_conflicts_open ON sync_conflicts(dismissed) WHERE dismissed = 0;

-- A simple key/value settings store. Settings are a synced entity class
-- (per-field LWW with audit, ADR-023); each key is one `settings` entity whose
-- `value` field is merged by clock. No canonical table existed before, so it is
-- introduced here alongside the sync machinery that replicates it.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY NOT NULL,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
) WITHOUT ROWID;
