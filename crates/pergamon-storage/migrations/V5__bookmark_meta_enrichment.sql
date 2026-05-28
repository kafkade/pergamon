-- V5: Add metadata enrichment columns to bookmark_meta.
-- Stores site name and favicon URL extracted during save.
ALTER TABLE bookmark_meta ADD COLUMN site_name TEXT;
ALTER TABLE bookmark_meta ADD COLUMN favicon_url TEXT;
