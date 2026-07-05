// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP routes for the sync server.

pub mod blobs;
pub mod events;
pub mod health;

use axum::Router;
use axum::routing::{get, post, put};

use crate::state::AppState;

/// Build the full application router (all endpoints, no middleware).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/v1/events", post(events::push).get(events::pull))
        .route("/v1/blobs/probe", post(blobs::probe))
        .route(
            "/v1/blobs/{account_id}/{ct_hash}",
            put(blobs::put).get(blobs::get),
        )
        .with_state(state)
}
