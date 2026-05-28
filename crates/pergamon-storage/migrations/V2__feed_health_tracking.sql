-- Feed health tracking and conditional GET support.
-- Adds columns needed for ETag/Last-Modified, error tracking, and sync state.

ALTER TABLE feeds ADD COLUMN etag TEXT;
ALTER TABLE feeds ADD COLUMN last_modified_header TEXT;
ALTER TABLE feeds ADD COLUMN error_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE feeds ADD COLUMN last_error TEXT;
ALTER TABLE feeds ADD COLUMN last_successful_fetch_at TEXT;
