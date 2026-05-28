-- V4: Add partial unique index on content_items.url for dedup.
-- Only enforces uniqueness where url IS NOT NULL.
CREATE UNIQUE INDEX IF NOT EXISTS idx_content_items_url_unique
ON content_items(url)
WHERE url IS NOT NULL;

-- Case-insensitive unique index on tag names.
-- Existing tags table uses TEXT for name; this enforces uniqueness.
CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_name_unique
ON tags(name COLLATE NOCASE);
