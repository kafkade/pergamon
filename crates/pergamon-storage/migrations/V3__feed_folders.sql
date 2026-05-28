-- Feed folders for organizing feed subscriptions (OPML categories).

CREATE TABLE IF NOT EXISTS feed_folders (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL,
    parent_id  TEXT REFERENCES feed_folders(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_feed_folders_parent ON feed_folders(parent_id);

-- Case-insensitive uniqueness within siblings (root folders).
CREATE UNIQUE INDEX idx_feed_folders_root_name
    ON feed_folders(name COLLATE NOCASE)
    WHERE parent_id IS NULL;

-- Case-insensitive uniqueness within siblings (child folders).
CREATE UNIQUE INDEX idx_feed_folders_child_name
    ON feed_folders(parent_id, name COLLATE NOCASE)
    WHERE parent_id IS NOT NULL;

-- Link feeds to their folder.
ALTER TABLE feeds ADD COLUMN folder_id TEXT REFERENCES feed_folders(id) ON DELETE SET NULL;
CREATE INDEX idx_feeds_folder ON feeds(folder_id);

-- Keep updated_at current on folder edits.
CREATE TRIGGER trg_feed_folders_updated_at
    AFTER UPDATE ON feed_folders
    FOR EACH ROW
    BEGIN
        UPDATE feed_folders
        SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE id = NEW.id;
    END;
