-- V14: one-time baseline backfill guard (issue #184)
--
-- Enabling remote sync on a device with an existing library must enqueue a
-- complete baseline (an Upsert for every pre-existing entity) so the first push
-- uploads the whole library, not just post-enable mutations. That backfill is a
-- one-time operation; this flag guards it so re-running is a no-op. It is
-- device-local engine bookkeeping (never synced), living beside the rest of the
-- single-row sync state.
ALTER TABLE sync_state ADD COLUMN baseline_done INTEGER NOT NULL DEFAULT 0;
