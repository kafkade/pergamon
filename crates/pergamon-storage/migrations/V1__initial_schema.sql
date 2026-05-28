-- pergamon initial schema
-- Implements the unified content model (ADR-002) and FTS5 strategy (ADR-006).

-- ============================================================
-- feeds — RSS/Atom/JSON Feed subscriptions
-- ============================================================
CREATE TABLE feeds (
    id          TEXT PRIMARY KEY NOT NULL,
    title       TEXT NOT NULL,
    url         TEXT NOT NULL UNIQUE,
    site_url    TEXT,
    description TEXT,
    last_fetched_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- ============================================================
-- content_items — unified table for all saved content (ADR-002)
-- ============================================================
CREATE TABLE content_items (
    id           TEXT PRIMARY KEY NOT NULL,
    url          TEXT,
    title        TEXT NOT NULL,
    author       TEXT,
    content_type TEXT NOT NULL
        CHECK (content_type IN (
            'feed_item', 'article', 'bookmark',
            'highlight', 'pdf', 'podcast_episode'
        )),
    status       TEXT NOT NULL DEFAULT 'inbox'
        CHECK (status IN (
            'inbox', 'later', 'reference',
            'reading', 'archived', 'discarded'
        )),
    content_text TEXT,
    excerpt      TEXT,
    published_at TEXT,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_content_items_type_status ON content_items(content_type, status);
CREATE INDEX idx_content_items_published   ON content_items(published_at);
CREATE INDEX idx_content_items_url         ON content_items(url);

-- ============================================================
-- Extension tables — type-specific metadata
-- ============================================================

-- Feed item metadata
CREATE TABLE feed_item_meta (
    content_item_id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_items(id) ON DELETE CASCADE,
    feed_id         TEXT NOT NULL
        REFERENCES feeds(id) ON DELETE CASCADE,
    guid            TEXT,
    summary         TEXT
);

-- Partial unique index: deduplicate feed items with non-null GUIDs
CREATE UNIQUE INDEX idx_feed_item_meta_feed_guid
    ON feed_item_meta(feed_id, guid)
    WHERE guid IS NOT NULL;

CREATE INDEX idx_feed_item_meta_feed ON feed_item_meta(feed_id);

-- Bookmark metadata
CREATE TABLE bookmark_meta (
    content_item_id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_items(id) ON DELETE CASCADE,
    original_url    TEXT,
    saved_from      TEXT,
    thumbnail_url   TEXT,
    description     TEXT
);

-- Highlight / annotation metadata
CREATE TABLE highlight_meta (
    content_item_id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_items(id) ON DELETE CASCADE,
    source_item_id  TEXT
        REFERENCES content_items(id) ON DELETE SET NULL,
    quote_text      TEXT NOT NULL,
    note            TEXT,
    position_start  INTEGER,
    position_end    INTEGER,
    color           TEXT
);

CREATE INDEX idx_highlight_meta_source ON highlight_meta(source_item_id);

-- ============================================================
-- Organization — collections and tags
-- ============================================================

CREATE TABLE collections (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL,
    parent_id  TEXT REFERENCES collections(id) ON DELETE SET NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE tags (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- Join tables (WITHOUT ROWID for composite-PK efficiency)

CREATE TABLE content_item_tags (
    content_item_id TEXT NOT NULL
        REFERENCES content_items(id) ON DELETE CASCADE,
    tag_id          TEXT NOT NULL
        REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (content_item_id, tag_id)
) WITHOUT ROWID;

CREATE INDEX idx_content_item_tags_tag ON content_item_tags(tag_id);

CREATE TABLE content_item_collections (
    content_item_id TEXT NOT NULL
        REFERENCES content_items(id) ON DELETE CASCADE,
    collection_id   TEXT NOT NULL
        REFERENCES collections(id) ON DELETE CASCADE,
    sort_order      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (content_item_id, collection_id)
) WITHOUT ROWID;

CREATE INDEX idx_content_item_collections_coll ON content_item_collections(collection_id);

-- ============================================================
-- FTS5 full-text search index (ADR-006)
-- ============================================================

CREATE VIRTUAL TABLE content_items_fts USING fts5(
    content_item_id UNINDEXED,
    title,
    author,
    content_text,
    tags
);

-- ============================================================
-- Triggers — keep updated_at current
-- ============================================================

CREATE TRIGGER trg_content_items_updated_at
    AFTER UPDATE ON content_items
    FOR EACH ROW
    BEGIN
        UPDATE content_items
        SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE id = NEW.id;
    END;

CREATE TRIGGER trg_feeds_updated_at
    AFTER UPDATE ON feeds
    FOR EACH ROW
    BEGIN
        UPDATE feeds
        SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE id = NEW.id;
    END;

CREATE TRIGGER trg_collections_updated_at
    AFTER UPDATE ON collections
    FOR EACH ROW
    BEGIN
        UPDATE collections
        SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE id = NEW.id;
    END;
