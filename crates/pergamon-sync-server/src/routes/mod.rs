// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP routes for the sync server.

pub mod blobs;
pub mod events;
pub mod health;
pub mod relay;

use axum::Router;
use axum::routing::{get, post, put};

use crate::abuse::{AbuseConfig, apply_strict_rate_limit, body_limit_layer};
use crate::state::AppState;

/// Build the full application router (all endpoints, no middleware).
///
/// This is the blind router today's ADR-026 behavior depends on, byte-for-byte:
/// no rate limiting, no body caps. It is used by [`crate::build_router`] and the
/// existing integration tests. The abuse-controlled equivalent is
/// [`hardened_router`].
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

/// Build the application router with per-route abuse controls (WP-4, #195).
///
/// This mirrors [`router`] but groups the routes so that:
/// - the sensitive **event-push** and **blob-upload** routes get the strict
///   per-IP rate-limit tier ([`apply_strict_rate_limit`]);
/// - control/JSON routes get the default body cap ([`AbuseConfig::max_body_bytes`]);
/// - the blob-upload route gets the larger upload cap
///   ([`AbuseConfig::upload_max_bytes`]);
/// - `/health` stays outside the strict tier so liveness probes are not throttled.
///
/// The **global** controls (default rate limit, concurrency/load-shed, and an
/// absolute body backstop) are layered on top by
/// [`crate::abuse::apply_abuse_controls`] at the serve site.
///
/// NOTE: the route table below must stay in sync with [`router`]; the split into
/// sub-routers is purely to scope middleware per route group.
pub fn hardened_router(state: AppState, abuse: &AbuseConfig) -> Router {
    // Sensitive: strict per-IP tier. Event pushes carry base64 ciphertext batches
    // and get the default body cap.
    let events = apply_strict_rate_limit(
        Router::new()
            .route("/v1/events", post(events::push).get(events::pull))
            .layer(body_limit_layer(abuse.max_body_bytes))
            .with_state(state.clone()),
        abuse,
    );

    // Sensitive: strict per-IP tier. Opaque blob uploads are the largest legit
    // body and get the upload cap.
    let blobs_rw = apply_strict_rate_limit(
        Router::new()
            .route(
                "/v1/blobs/{account_id}/{ct_hash}",
                put(blobs::put).get(blobs::get),
            )
            .layer(body_limit_layer(abuse.upload_max_bytes))
            .with_state(state.clone()),
        abuse,
    );

    // Everything else: default body cap only (`/health` is a bodyless GET). Kept
    // off the strict tier so liveness probes and dedup/relay reads stay cheap.
    let base = Router::new()
        .route("/health", get(health::health))
        .route("/v1/blobs/probe", post(blobs::probe))
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
        .layer(body_limit_layer(abuse.max_body_bytes))
        .with_state(state);

    base.merge(events).merge(blobs_rw)
}
