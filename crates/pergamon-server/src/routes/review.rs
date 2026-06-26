// SPDX-License-Identifier: AGPL-3.0-only

//! Spaced-repetition (FSRS) review API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::fsrs::{MemoryState, Parameters, Rating, Scheduler};
use pergamon_core::model::{ReviewCard, ReviewLog, ReviewStatsReport};

use crate::error::ApiError;
use crate::state::AppState;

// ======================================================================
// Query / request types
// ======================================================================

/// Query parameters for the review queue.
#[derive(Debug, Deserialize)]
pub struct QueueQuery {
    /// Maximum number of due cards to return.
    pub limit: Option<usize>,
}

/// Request body for submitting a review.
#[derive(Debug, Deserialize)]
pub struct SubmitReviewRequest {
    /// Rating: `again`, `hard`, `good`, or `easy`.
    pub rating: Rating,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/review/queue` — list due review cards.
pub async fn review_queue(
    State(state): State<AppState>,
    Query(query): Query<QueueQuery>,
) -> Result<Json<Vec<ReviewCard>>, ApiError> {
    let now = OffsetDateTime::now_utc();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let mut cards = db.list_due_review_cards(now)?;
    if let Some(max) = query.limit {
        cards.truncate(max);
    }
    Ok(Json(cards))
}

/// `POST /api/review/{card_id}` — submit a review rating for a card.
///
/// Runs the FSRS scheduler, persists the updated card state, and records a
/// review log entry. Returns the updated card.
pub async fn submit_review(
    State(state): State<AppState>,
    Path(card_id): Path<Uuid>,
    Json(body): Json<SubmitReviewRequest>,
) -> Result<Json<ReviewCard>, ApiError> {
    let rating = body.rating;
    let now = OffsetDateTime::now_utc();

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    let card = db
        .get_review_card(card_id)
        .map_err(|_| ApiError::not_found("review card not found"))?;

    let scheduler = Scheduler::new(&Parameters::default());

    let elapsed_days = card.last_reviewed_at.map_or(0.0, |last| {
        let dur = now - last;
        dur.as_seconds_f64() / 86_400.0
    });

    let memory = match (card.stability, card.difficulty) {
        (Some(s), Some(d)) => Some(MemoryState {
            stability: s,
            difficulty: d,
        }),
        _ => None,
    };

    let output = scheduler.schedule(card.state, memory, elapsed_days, rating);

    let due_at = now + time::Duration::seconds_f64(output.scheduled_days * 86_400.0);
    let new_review_count = card.review_count + 1;
    let new_lapse_count = if rating == Rating::Again {
        card.lapse_count + 1
    } else {
        card.lapse_count
    };

    db.update_review_card(
        card.id,
        output.next_state.as_str(),
        output.memory.stability,
        output.memory.difficulty,
        due_at,
        now,
        new_review_count,
        new_lapse_count,
        output.scheduled_days,
    )?;

    let log = ReviewLog {
        id: Uuid::new_v4(),
        card_id: card.id,
        rating,
        state_before: card.state,
        stability_before: card.stability,
        difficulty_before: card.difficulty,
        state_after: output.next_state,
        stability_after: output.memory.stability,
        difficulty_after: output.memory.difficulty,
        elapsed_days,
        scheduled_days: output.scheduled_days,
        reviewed_at: now,
    };
    db.insert_review_log(&log)?;

    let updated = db.get_review_card(card.id)?;
    Ok(Json(updated))
}

/// `GET /api/review/stats` — review statistics report.
pub async fn review_stats(
    State(state): State<AppState>,
) -> Result<Json<ReviewStatsReport>, ApiError> {
    let now = OffsetDateTime::now_utc();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let report = db.review_stats_report(now)?;
    Ok(Json(report))
}
