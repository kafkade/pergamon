// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP routes for the sync server.

pub mod blobs;
pub mod events;
pub mod health;
pub mod relay;

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
        // Opaque onboarding-artifact relay (ADR-024, #125).
        .route("/v1/devices/{account_id}", get(relay::devices_list))
        .route(
            "/v1/devices/{account_id}/{device_id}",
            put(relay::device_put).get(relay::device_get),
        )
        .route(
            "/v1/wraps/{account_id}/{device_id}",
            post(relay::wrap_put).get(relay::wraps_list),
        )
        .route(
            "/v1/attestations/{account_id}",
            post(relay::attestation_append).get(relay::attestations_list),
        )
        .route(
            "/v1/recovery/{account_id}",
            put(relay::recovery_put).get(relay::recovery_get),
        )
        .with_state(state)
}
