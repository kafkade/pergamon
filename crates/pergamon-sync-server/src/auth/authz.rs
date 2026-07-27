// SPDX-License-Identifier: AGPL-3.0-only

//! Per-route authorization + tenant isolation (WP-3c, #197).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! This module turns the WP-3b [`AuthState::validate_token`] primitive into the
//! hard multi-tenant boundary: in multi-tenant mode **every** account-scoped
//! content/relay route is gated so that it is only reachable with a valid bearer
//! token whose `account_id` equals the route's target `account_id`.
//!
//! ## Two enforcement points, one invariant
//! The target `account_id` lives in three different places depending on the
//! route, so enforcement is split — but the invariant is uniform: *no
//! account-scoped route is reachable unless a valid token's `account_id` equals
//! the target `account_id`.*
//!
//! 1. **Authentication + path-param authorization** — [`require_account_auth`],
//!    an [`axum::middleware::from_fn_with_state`] layer applied (in
//!    [`crate::build_router_multitenant`] /
//!    [`crate::build_router_multitenant_hardened`]) to the content routes
//!    **before** the OPAQUE auth router is merged. It validates the bearer once
//!    (401 on any failure), and for routes that carry `{account_id}` in the
//!    **path** it also does the 403 tenant compare centrally. It then injects the
//!    resolved [`AuthAccount`] into the request extensions.
//! 2. **Body/query authorization** — for the three routes whose `account_id` is
//!    in the request **body** (`POST /v1/events`, `POST /v1/blobs/probe`) or
//!    **query** (`GET /v1/events`), the handler pulls the injected
//!    [`AuthAccount`] (`Option<Extension<AuthAccount>>`) and calls
//!    [`authorize_account`] against the body/query `account_id`. In blind mode the
//!    extension is absent (`None`), so those handlers are behavior-identical.
//!
//! ## 401 vs 403
//! - **401** (`UNAUTHORIZED`): the caller could not be authenticated —
//!   missing/blank/malformed header, or an unknown/expired/revoked token. The
//!   body is uniform and never distinguishes which (no token/account-existence
//!   oracle).
//! - **403** ([`ApiError::forbidden`]): the caller authenticated but targeted a
//!   different tenant. Every 403 is paired with a structured audit
//!   [`tracing::warn!`] on the `pergamon::auth::audit` target.
//!
//! ## Account-id namespace (verified)
//! The `account_id` a token carries is the server-allocated 128-bit handle minted
//! at OPAQUE `register/finish`; the blind content plane keys on that **same** id.
//! A direct equality check is therefore correct and complete.
//!
//! ## Seams left for later work (do not enforce here)
//! - **Per-device roster membership is deliberately NOT required.** ADR-029
//!   join-flow C: a freshly-authenticated, not-yet-SAS-enrolled device holds a
//!   valid token yet must still be able to READ the roster/wraps to get enrolled.
//!   We enforce **account** isolation only; gating a device on prior
//!   `device_records` membership is an onboarding/WP-3c follow-up seam.
//! - Per-tenant quotas → WP-3d (#198). Connection pool / WAL / per-tenant
//!   fairness → WP-3e (#201). A **persistent** audit table (this module only
//!   emits a structured log event) → future enhancement.

use axum::body::Body;
use axum::extract::{FromRequestParts as _, RawPathParams, State};
use axum::http::{Request, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::state::AuthState;
use crate::auth::token::AuthAccount;
use crate::error::ApiError;

/// The one liveness route that stays open in multi-tenant mode (no bearer).
const HEALTH_PATH: &str = "/health";

/// The path-param name that carries the target tenant on the `{account_id}`
/// routes. Kept in one place so the middleware and the route table cannot drift.
const ACCOUNT_ID_PARAM: &str = "account_id";

/// Extract the raw bearer credential from an `Authorization: Bearer <...>`
/// header value.
///
/// Returns `None` for a missing header, a non-ASCII value, a non-`Bearer`
/// scheme, or an empty credential — all of which the caller maps to a uniform
/// 401.
fn bearer_from_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    // Case-insensitive scheme match, single required space (RFC 7235 shape).
    let (scheme, credential) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let credential = credential.trim();
    if credential.is_empty() {
        return None;
    }
    Some(credential.to_string())
}

/// Authenticate the bearer and enforce tenant isolation for account-scoped
/// content routes (WP-3c, #197).
///
/// Mounted only by the multi-tenant router builders and only over the content
/// routes (never `/v1/auth/*`, which establish identity and precede any bearer).
/// `/health` is allowlisted so liveness probes never need a token.
///
/// On success the resolved [`AuthAccount`] is inserted into the request
/// extensions so the body/query handlers can complete their own tenant compare
/// via [`authorize_account`].
pub async fn require_account_auth(
    State(auth_state): State<AuthState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Liveness stays open even in multi-tenant mode.
    if req.uri().path() == HEALTH_PATH {
        return next.run(req).await;
    }

    // 1. Authenticate: a valid, unexpired, unrevoked bearer or a uniform 401.
    let Some(bearer) = bearer_from_header(req.headers().get(header::AUTHORIZATION)) else {
        return ApiError::unauthorized("missing or malformed bearer token").into_response();
    };
    let account = match auth_state.validate_token(&bearer) {
        Ok(Some(account)) => account,
        // Unknown / expired / revoked / malformed all collapse to one 401.
        Ok(None) => {
            return ApiError::unauthorized("invalid or expired bearer token").into_response();
        }
        Err(err) => return err.into_response(),
    };

    // 2. Authorize the path-param routes centrally. Split into parts so we can
    //    read the matched `{account_id}` (if any) without touching the body.
    let (mut parts, body) = req.into_parts();
    let method = parts.method.clone();
    let path = parts.uri.path().to_string();
    let target = RawPathParams::from_request_parts(&mut parts, &())
        .await
        .ok()
        .and_then(|params| {
            params
                .iter()
                .find(|(name, _)| *name == ACCOUNT_ID_PARAM)
                .map(|(_, value)| value.to_string())
        });
    if let Some(target) = target
        && let Err(err) = authorize_account(&account, &target, method.as_str(), &path)
    {
        return err.into_response();
    }

    // 3. Hand the authenticated principal to the body/query handlers.
    let mut req = Request::from_parts(parts, body);
    req.extensions_mut().insert(account);
    next.run(req).await
}

/// Assert an authenticated principal is authorized for `target` and, on a
/// mismatch, emit the WP-3c audit event and a 403.
///
/// This is the single tenant-isolation compare shared by the middleware (for
/// path-param routes) and the three body/query handlers. On mismatch it logs a
/// structured [`tracing::warn!`] on the `pergamon::auth::audit` target carrying
/// the authenticated `account_id`, the target `account_id`, the `device_id`, the
/// HTTP method, and the route path, then returns [`ApiError::forbidden`].
///
/// # Errors
/// Returns [`ApiError::forbidden`] (403) when `auth.account_id != target`.
pub fn authorize_account(
    auth: &AuthAccount,
    target: &str,
    method: &str,
    path: &str,
) -> Result<(), ApiError> {
    if auth.account_id == target {
        return Ok(());
    }
    tracing::warn!(
        target: "pergamon::auth::audit",
        authenticated_account_id = %auth.account_id,
        target_account_id = %target,
        device_id = %auth.device_id,
        http_method = %method,
        route_path = %path,
        "cross-tenant access denied",
    );
    Err(ApiError::forbidden(
        "token is not authorized for this account",
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]

    use axum::http::HeaderValue;

    use super::*;

    fn account(id: &str) -> AuthAccount {
        AuthAccount {
            account_id: id.to_string(),
            device_id: "device-abc".to_string(),
        }
    }

    #[test]
    fn bearer_parsing_accepts_valid_and_rejects_junk() {
        assert_eq!(
            bearer_from_header(Some(&HeaderValue::from_static("Bearer tok.secret"))),
            Some("tok.secret".to_string())
        );
        // Scheme is case-insensitive.
        assert_eq!(
            bearer_from_header(Some(&HeaderValue::from_static("bearer tok.secret"))),
            Some("tok.secret".to_string())
        );
        // Missing header, wrong scheme, and empty credential all fail.
        assert_eq!(bearer_from_header(None), None);
        assert_eq!(
            bearer_from_header(Some(&HeaderValue::from_static("Basic tok"))),
            None
        );
        assert_eq!(
            bearer_from_header(Some(&HeaderValue::from_static("Bearer   "))),
            None
        );
        assert_eq!(
            bearer_from_header(Some(&HeaderValue::from_static("tok-no-scheme"))),
            None
        );
    }

    #[test]
    fn authorize_account_allows_same_tenant_and_denies_cross_tenant() {
        let alice = account("acct-alice");
        assert!(authorize_account(&alice, "acct-alice", "GET", "/v1/devices/acct-alice").is_ok());

        let err = authorize_account(&alice, "acct-bob", "GET", "/v1/devices/acct-bob")
            .expect_err("cross-tenant must be denied");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(err.code, "FORBIDDEN");
    }
}
