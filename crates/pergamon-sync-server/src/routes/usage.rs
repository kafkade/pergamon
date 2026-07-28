// SPDX-License-Identifier: AGPL-3.0-only

//! Per-tenant usage/metrics endpoint (WP-3d, [#198]).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! Exposes an account's metered ciphertext usage for billing/metrics. It reports
//! **sizes and object counts only** — never any decoded content — so it upholds
//! the blind-relay invariant (ADR-026, design §2.5). The `account_id` is a
//! **path** parameter, so in multi-tenant mode the WP-3c authorization middleware
//! ([`crate::auth::require_account_auth`]) authenticates the bearer and asserts
//! `token.account_id == {account_id}` before this handler runs; a cross-tenant
//! request is a `403` and a missing/invalid token a `401`. In blind mode there is
//! no auth layer and the route is open, exactly like the rest of the relay.
//!
//! An operator-wide aggregate / Prometheus `/metrics` endpoint is intentionally
//! **out of scope** here (no operator-auth surface exists yet) — a future seam.
//!
//! [#198]: https://github.com/kafkade/pergamon/issues/198

use axum::Json;
use axum::extract::{Path, State};

use crate::envelope::UsageResponse;
use crate::error::ApiError;
use crate::state::AppState;

/// Report an account's metered ciphertext usage and quota status
/// (`GET /v1/usage/{account_id}`).
///
/// Sums live ciphertext sizes/counts from the content store (WP-3d A1
/// accounting) and pairs them with the server's configured caps. Tenant
/// isolation is enforced upstream by the path-param authorization gate; see the
/// module docs.
///
/// # Errors
/// Returns 401/403 in multi-tenant mode when the caller is unauthenticated or
/// targets another tenant (handled by the middleware); 500 on a store failure.
pub async fn get(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
) -> Result<Json<UsageResponse>, ApiError> {
    let (usage, quota) = {
        let store = state.lock_store()?;
        (store.account_usage(&account_id)?, store.quota())
    };

    let total_bytes = usage.total_bytes();
    let total_objects = usage.total_objects();
    let over_quota = quota.check(total_bytes, total_objects).is_err();

    Ok(Json(UsageResponse {
        blob_bytes: usage.blob_bytes,
        blob_count: usage.blob_count,
        event_bytes: usage.event_bytes,
        event_count: usage.event_count,
        total_bytes,
        total_objects,
        max_account_bytes: quota.max_account_bytes,
        max_account_objects: quota.max_account_objects,
        over_quota,
    }))
}
