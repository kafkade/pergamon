-- Content rules: automatic organization of content items.
--
-- Each rule has a filter query (same DSL as smart collections) and
-- a JSON array of actions to apply when the filter matches.

CREATE TABLE content_rules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 0,
    filter_query TEXT NOT NULL,
    actions_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
