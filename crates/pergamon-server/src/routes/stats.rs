// SPDX-License-Identifier: AGPL-3.0-only

//! Statistics API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::State;
use time::OffsetDateTime;

use pergamon_core::model::{ReviewStatsReport, UsageStatsReport};

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/stats/usage` — usage / reading statistics report.
pub async fn usage_stats(
    State(state): State<AppState>,
) -> Result<Json<UsageStatsReport>, ApiError> {
    let now = OffsetDateTime::now_utc();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let report = db.usage_stats_report(now)?;
    Ok(Json(report))
}

/// `GET /api/stats/review` — review / retention statistics report.
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
