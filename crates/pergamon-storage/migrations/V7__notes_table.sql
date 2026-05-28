-- Notes table — free-form annotations attached to any content item.
-- Implements the note entity from issue #18.

CREATE TABLE notes (
    id              TEXT PRIMARY KEY NOT NULL,
    content_item_id TEXT NOT NULL
        REFERENCES content_items(id) ON DELETE CASCADE,
    body            TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_notes_content_item ON notes(content_item_id);

CREATE TRIGGER trg_notes_updated_at
    AFTER UPDATE ON notes
    FOR EACH ROW
    BEGIN
        UPDATE notes
        SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE id = NEW.id;
    END;
