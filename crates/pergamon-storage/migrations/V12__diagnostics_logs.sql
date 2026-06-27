-- Diagnostics logs: back the admin diagnostics view.
--
-- `import_log` records one row per import run (OPML, Raindrop, Pocket, Kindle,
-- Readwise, backup) with item counts and any error detail. `extraction_log`
-- records one row per content-extraction attempt (CLI save, server bookmark
-- add, feed sync) with success/failure and the extractor used.

CREATE TABLE import_log (
    id             TEXT PRIMARY KEY NOT NULL,
    source         TEXT NOT NULL
        CHECK (source IN (
            'opml', 'raindrop', 'pocket', 'kindle', 'readwise', 'backup'
        )),
    file_name      TEXT,
    items_added    INTEGER NOT NULL DEFAULT 0,
    items_existing INTEGER NOT NULL DEFAULT 0,
    items_skipped  INTEGER NOT NULL DEFAULT 0,
    errors         INTEGER NOT NULL DEFAULT 0,
    error_detail   TEXT,
    dry_run        INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_import_log_created_at ON import_log(created_at);

CREATE TABLE extraction_log (
    id              TEXT PRIMARY KEY NOT NULL,
    content_item_id TEXT REFERENCES content_items(id) ON DELETE SET NULL,
    url             TEXT,
    source          TEXT NOT NULL
        CHECK (source IN ('save', 'feed_sync', 'bookmark')),
    success         INTEGER NOT NULL,
    extractor       TEXT,
    error_message   TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_extraction_log_created_at ON extraction_log(created_at);
CREATE INDEX idx_extraction_log_success ON extraction_log(success);
