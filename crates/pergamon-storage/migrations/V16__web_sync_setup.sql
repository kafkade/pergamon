-- Local operator state, deliberately separate from synced settings and V15 authority.
CREATE TABLE web_sync_setup (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    relay_url TEXT NOT NULL,
    identity_handle TEXT NOT NULL,
    flow TEXT NOT NULL CHECK (flow IN ('create', 'attach', 'join')),
    phase TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    recovery_ack INTEGER NOT NULL DEFAULT 0 CHECK (recovery_ack IN (0, 1)),
    publication_millis INTEGER NOT NULL,
    approver_device_id TEXT
);

CREATE TABLE web_sync_runtime (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    needs_login INTEGER NOT NULL DEFAULT 0 CHECK (needs_login IN (0, 1))
);
INSERT INTO web_sync_runtime (id, needs_login) VALUES (1, 0);
