// SPDX-License-Identifier: AGPL-3.0-only

//! Health check endpoint.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::state::AppState;

/// JSON response body for the health check endpoint.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Server health status (`ok` or `error`).
    status: &'static str,
    /// Crate version from `Cargo.toml`.
    version: &'static str,
}

/// Health check endpoint (`GET /health`).
///
/// Returns HTTP 200 with server status and version when the database is
/// accessible. Returns HTTP 503 if the database lock is poisoned.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let (status_code, status_str) = if state.db.lock().is_ok() {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "error")
    };
    let body = HealthResponse {
        status: status_str,
        version: env!("CARGO_PKG_VERSION"),
    };
    (status_code, Json(body)).into_response()
}
