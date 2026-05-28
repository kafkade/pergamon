-- V6: Link health tracking for dead link detection.
-- Stores HTTP status, redirect information, and error details
-- for each checked content item URL.
CREATE TABLE link_health (
    content_item_id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_items(id) ON DELETE CASCADE,
    http_status     INTEGER,
    final_url       TEXT,
    redirect_count  INTEGER NOT NULL DEFAULT 0,
    last_checked_at TEXT NOT NULL,
    error_message   TEXT
);

CREATE INDEX idx_link_health_status ON link_health(http_status);
CREATE INDEX idx_link_health_checked ON link_health(last_checked_at);
