-- Add read_at timestamp to content_items for reading analytics.
-- Set only when an item transitions to 'archived' status.
ALTER TABLE content_items ADD COLUMN read_at TEXT;

-- Backfill: use updated_at for already-archived items.
UPDATE content_items SET read_at = updated_at WHERE status = 'archived' AND read_at IS NULL;

-- Index for analytics queries that filter by read_at date.
CREATE INDEX idx_content_items_read_at ON content_items(read_at);
