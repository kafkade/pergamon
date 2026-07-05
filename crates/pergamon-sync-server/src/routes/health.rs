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
/// Returns HTTP 200 when the store lock is acquirable, or HTTP 503 if it is
/// poisoned.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let (code, status) = if state.store.lock().is_ok() {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "error")
    };
    let body = HealthResponse {
        status,
        version: env!("CARGO_PKG_VERSION"),
    };
    (code, Json(body))
}
