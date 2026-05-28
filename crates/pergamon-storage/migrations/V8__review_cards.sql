-- Review cards: FSRS spaced repetition state per highlight.
-- Each highlight can have at most one review card.
CREATE TABLE review_cards (
    id               TEXT PRIMARY KEY NOT NULL,
    content_item_id  TEXT NOT NULL UNIQUE
        REFERENCES highlight_meta(content_item_id) ON DELETE CASCADE,
    state            TEXT NOT NULL DEFAULT 'new'
        CHECK (state IN ('new', 'learning', 'review', 'relearning')),
    stability        REAL,
    difficulty       REAL,
    due_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_reviewed_at TEXT,
    review_count     INTEGER NOT NULL DEFAULT 0,
    lapse_count      INTEGER NOT NULL DEFAULT 0,
    scheduled_days   REAL,
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_review_cards_due     ON review_cards(due_at);
CREATE INDEX idx_review_cards_state   ON review_cards(state);

CREATE TRIGGER trg_review_cards_updated_at
    AFTER UPDATE ON review_cards
    FOR EACH ROW
    BEGIN
        UPDATE review_cards
        SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE id = NEW.id;
    END;

-- Review logs: immutable record of every review event.
CREATE TABLE review_logs (
    id                 TEXT PRIMARY KEY NOT NULL,
    card_id            TEXT NOT NULL
        REFERENCES review_cards(id) ON DELETE CASCADE,
    rating             INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 4),
    state_before       TEXT NOT NULL,
    stability_before   REAL,
    difficulty_before  REAL,
    state_after        TEXT NOT NULL,
    stability_after    REAL NOT NULL,
    difficulty_after   REAL NOT NULL,
    elapsed_days       REAL NOT NULL,
    scheduled_days     REAL NOT NULL,
    reviewed_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_review_logs_card       ON review_logs(card_id);
CREATE INDEX idx_review_logs_reviewed   ON review_logs(reviewed_at);
