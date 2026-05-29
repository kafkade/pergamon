-- Smart collections: auto-populated collections driven by a filter query.
--
-- Adds two columns to the existing collections table:
--   is_smart      — flag distinguishing smart from manual collections
--   filter_query  — the DSL filter string (e.g. "type:article tag:rust")
--
-- CHECK: smart collections must have a filter_query; manual ones must not.

ALTER TABLE collections ADD COLUMN is_smart INTEGER NOT NULL DEFAULT 0;
ALTER TABLE collections ADD COLUMN filter_query TEXT;

-- Enforce invariant: smart ⟺ filter_query is present.
-- SQLite CHECK on ALTER is not supported, so we use a trigger pair.

CREATE TRIGGER trg_collections_smart_insert
    BEFORE INSERT ON collections
    FOR EACH ROW
    WHEN (NEW.is_smart = 1 AND NEW.filter_query IS NULL)
         OR (NEW.is_smart = 0 AND NEW.filter_query IS NOT NULL)
    BEGIN
        SELECT RAISE(ABORT, 'smart collections must have a filter_query; manual collections must not');
    END;

CREATE TRIGGER trg_collections_smart_update
    BEFORE UPDATE ON collections
    FOR EACH ROW
    WHEN (NEW.is_smart = 1 AND NEW.filter_query IS NULL)
         OR (NEW.is_smart = 0 AND NEW.filter_query IS NOT NULL)
    BEGIN
        SELECT RAISE(ABORT, 'smart collections must have a filter_query; manual collections must not');
    END;
