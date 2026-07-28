// SPDX-License-Identifier: AGPL-3.0-only

//! Pre-auth abuse controls for the AGPL sync server (WP-4, [#195]).
//!
//! The blind relay (ADR-026) ships with **no built-in authentication**: access
//! control is an operational concern handled by a reverse proxy. End-to-end
//! encryption protects the *confidentiality* of content but not the
//! *availability* of the service — an unauthenticated or newly-registered caller
//! can still exhaust the relay by flooding it with requests, oversized bodies, or
//! concurrent uploads (ADR-026 "Negative" §). This module adds composable,
//! safe-by-default transport middleware to bound that abuse:
//!
//! - **Per-IP rate limiting** (two tiers: a generous default and a stricter tier
//!   for sensitive routes) via `tower_governor` (GCRA, backed by `governor`).
//! - **Request/upload body-size caps** via `tower_http`'s `RequestBodyLimitLayer`.
//! - **Storage-DoS isolation**: a global concurrency limit with load-shedding so
//!   a single flooder cannot make the process queue unboundedly.
//!
//! Everything is driven by [`AbuseConfig`], whose defaults are deliberately
//! generous so a normal single-account self-host (whose first sync uploads a
//! whole library) is unaffected; the controls bound *exhaustion*, not legit use.
//!
//! ## Client-IP determination and the reverse-proxy trust assumption
//! By default the rate limiter keys on the **socket peer IP**. This is the safe
//! choice for a directly-exposed relay: it cannot be bypassed by spoofing an
//! `X-Forwarded-For` header. ADR-026 puts a reverse proxy in front of the relay,
//! in which case the peer IP is always the proxy's IP (so per-IP limiting would
//! lump every client together). Operators running behind a **trusted** proxy
//! should therefore set [`AbuseConfig::trust_proxy_headers`] to key on the real
//! client IP from `X-Forwarded-For` / `X-Real-Ip` / `Forwarded` (falling back to
//! the peer IP). Only enable this when the proxy is trusted to set those headers,
//! otherwise any caller can forge its apparent IP.
//!
//! ## What the storage-DoS isolation does and does NOT guarantee
//! The global concurrency + load-shed layer caps total in-flight work, so one
//! IP/tenant flooding uploads cannot drive unbounded queueing/OOM — excess is
//! shed cleanly with `503`. Combined with per-IP rate limiting, a single source
//! is bounded. It does **not** by itself provide per-tenant *fairness*: it
//! counts requests, not tenants.
//!
//! That gap is now closed one layer down. WP-3e ([#201]) replaced the single
//! `Arc<Mutex<SyncStore>>` this module originally called out with a WAL database
//! behind one writer connection and a bounded reader pool
//! ([`crate::pool`]), and added a per-`account_id` in-flight cap
//! ([`crate::fairness`]) so one heavy tenant cannot hold every pooled
//! connection. The division of labour is: **this** module bounds aggregate load
//! and per-IP rate; [`crate::fairness`] bounds per-tenant concurrency;
//! [`crate::quota`] bounds per-tenant storage. None of them makes `SQLite` accept
//! more than one writer at a time — see ADR-031 for that ceiling.

//!
//! ## Reusable strict tier
//! [`apply_strict_rate_limit`] is a public, composable hook intended for
//! sensitive endpoints (registration/login/upload/event-push). It is applied to
//! the routes that exist today in [`crate::routes::hardened_router`] and to the
//! OPAQUE auth control plane in [`crate::build_router_multitenant_hardened`], and
//! is the seam any future auth-adjacent work package should adopt.
//!
//! [#195]: https://github.com/kafkade/pergamon/issues/195
//! [#201]: https://github.com/kafkade/pergamon/issues/201

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::error_handling::HandleErrorLayer;
use axum::http::{HeaderValue, Request, header};
use axum::response::{IntoResponse, Response};
use tower::load_shed::error::Overloaded;
use tower::{BoxError, ServiceBuilder};
use tower_governor::GovernorError;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::{KeyExtractor, PeerIpKeyExtractor, SmartIpKeyExtractor};
use tower_http::limit::RequestBodyLimitLayer;

use crate::error::ApiError;

/// One mebibyte, for readable byte-size defaults.
const MIB: usize = 1024 * 1024;

/// How often the background task prunes idle per-IP rate-limiter buckets, bounding
/// limiter memory against IP-spray (see [`AbuseConfig::max_concurrency`] rationale).
#[allow(clippy::duration_suboptimal_units)] // 60s reads clearer than 1min here.
const LIMITER_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Configuration for the pre-auth abuse controls.
///
/// All fields are surfaced as CLI flags / environment variables by the server
/// binary. Defaults ([`AbuseConfig::default`]) are conservative-but-generous so a
/// normal self-host is unaffected; operators tune them down for hostile exposure.
///
/// A rate-limit tier is **disabled** when its `rps` or `burst` is `0` (the layer
/// is simply not attached); [`max_concurrency`](Self::max_concurrency) `== 0`
/// likewise disables the concurrency/load-shed layer.
#[derive(Debug, Clone)]
pub struct AbuseConfig {
    /// Sustained per-IP request rate for the **default** tier, in requests/second
    /// (one token is replenished every `1/rps` seconds). `0` disables the tier.
    pub rate_limit_rps: u32,
    /// Maximum instantaneous burst for the **default** tier. `0` disables the tier.
    pub rate_limit_burst: u32,
    /// Sustained per-IP request rate for the **strict** tier (sensitive routes),
    /// in requests/second. `0` disables the tier.
    pub strict_rate_limit_rps: u32,
    /// Maximum instantaneous burst for the **strict** tier. `0` disables the tier.
    pub strict_rate_limit_burst: u32,
    /// Default maximum request body size, in bytes, for control/JSON routes.
    pub max_body_bytes: usize,
    /// Maximum request body size, in bytes, for the opaque blob-upload route and
    /// the global backstop. This is the largest legitimate request the relay
    /// accepts; it should be `>= max_body_bytes`.
    pub upload_max_bytes: usize,
    /// Maximum number of requests processed concurrently before excess is
    /// load-shed with `503` (storage-DoS isolation). `0` disables the layer.
    pub max_concurrency: usize,
    /// Trust reverse-proxy client-IP headers (`X-Forwarded-For` / `X-Real-Ip` /
    /// `Forwarded`) instead of the socket peer IP. Only enable behind a trusted
    /// proxy — see the module docs.
    pub trust_proxy_headers: bool,
}

impl Default for AbuseConfig {
    fn default() -> Self {
        Self {
            // Generous per-IP defaults: a single client bursting a handful of
            // requests during sync is unaffected, but a flooder is bounded.
            rate_limit_rps: 50,
            rate_limit_burst: 100,
            // Stricter tier for registration/login/upload/event-push.
            strict_rate_limit_rps: 20,
            strict_rate_limit_burst: 40,
            // Control/JSON routes; event pushes carry base64 ciphertext batches.
            max_body_bytes: 16 * MIB,
            // Opaque blobs (article snapshots, PDFs) are the largest legit body.
            upload_max_bytes: 64 * MIB,
            // Pooled, WAL-backed SQLite ops; ample headroom for a self-host.
            max_concurrency: 256,
            // Secure default: key on the socket peer IP (no header spoofing).
            trust_proxy_headers: false,
        }
    }
}

/// A [`KeyExtractor`] that keys rate limiting on the client IP address.
///
/// When [`trust_proxy_headers`](AbuseConfig::trust_proxy_headers) is `false` (the
/// default) it uses the socket peer IP ([`PeerIpKeyExtractor`]); otherwise it
/// trusts the proxy client-IP headers ([`SmartIpKeyExtractor`]), falling back to
/// the peer IP. See the module docs for the reverse-proxy trust assumption.
#[derive(Debug, Clone, Copy)]
pub struct IpKeyExtractor {
    /// Whether to trust reverse-proxy client-IP headers.
    trust_proxy_headers: bool,
}

impl KeyExtractor for IpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        if self.trust_proxy_headers {
            SmartIpKeyExtractor.extract(req)
        } else {
            PeerIpKeyExtractor.extract(req)
        }
    }
}

/// Render a [`GovernorError`] as a uniform, non-leaky [`ApiError`] response.
///
/// A rate-limit rejection becomes `429` with a `Retry-After` header; a failure to
/// determine the client IP (a deployment misconfiguration, e.g. missing
/// `ConnectInfo`) becomes an opaque `500`.
///
/// Takes the error by value to match `tower_governor`'s `error_handler` callback
/// contract (`Fn(GovernorError) -> Response`).
#[allow(clippy::needless_pass_by_value)]
fn governor_error_response(err: GovernorError) -> Response {
    match err {
        GovernorError::TooManyRequests { wait_time, .. } => {
            let mut response = ApiError::too_many_requests("rate limit exceeded").into_response();
            if let Ok(value) = HeaderValue::from_str(&wait_time.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
        GovernorError::UnableToExtractKey => {
            ApiError::internal("unable to determine client address").into_response()
        }
        GovernorError::Other { .. } => {
            ApiError::too_many_requests("rate limit exceeded").into_response()
        }
    }
}

/// Attach a per-IP rate-limit layer to `router` for the given rate/burst.
///
/// Returns `router` unchanged when the tier is disabled (`rps == 0 || burst == 0`)
/// so no [`ConnectInfo`](axum::extract::ConnectInfo) is required in that case.
/// When enabled, a background task periodically prunes idle buckets to bound
/// limiter memory against IP-spray.
fn apply_rate_limit(router: Router, rps: u32, burst: u32, trust_proxy_headers: bool) -> Router {
    if rps == 0 || burst == 0 {
        return router;
    }

    // Replenish one token every `1/rps` seconds.
    let period = Duration::from_secs(1) / rps;
    let mut builder = GovernorConfigBuilder::default();
    builder.period(period).burst_size(burst);
    let mut builder = builder.key_extractor(IpKeyExtractor {
        trust_proxy_headers,
    });
    let Some(config) = builder.finish() else {
        // Unreachable given the guard above, but stay defensive rather than panic.
        return router;
    };

    let config = Arc::new(config);

    // Bound limiter memory: prune idle per-IP buckets on an interval. Only spawn
    // when a Tokio runtime is present (it always is under `axum::serve`); tests
    // that build a router without a runtime simply skip the cleanup task.
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let cleanup = Arc::clone(&config);
        handle.spawn(async move {
            let mut ticker = tokio::time::interval(LIMITER_CLEANUP_INTERVAL);
            loop {
                ticker.tick().await;
                cleanup.limiter().retain_recent();
            }
        });
    }

    let layer = GovernorLayer::new(config).error_handler(governor_error_response);
    router.layer(layer)
}

/// Build a request body-size cap layer.
#[must_use]
pub fn body_limit_layer(max_bytes: usize) -> RequestBodyLimitLayer {
    RequestBodyLimitLayer::new(max_bytes)
}

/// Apply the **default** per-IP rate-limit tier to `router`.
pub fn apply_default_rate_limit(router: Router, config: &AbuseConfig) -> Router {
    apply_rate_limit(
        router,
        config.rate_limit_rps,
        config.rate_limit_burst,
        config.trust_proxy_headers,
    )
}

/// Apply the **strict** per-IP rate-limit tier to `router`.
///
/// This is the reusable hook for sensitive endpoints (registration/login/upload/
/// event-push). Applying it to a whole sub-router keeps the same limit across all
/// of that sub-router's methods; see the module docs.
pub fn apply_strict_rate_limit(router: Router, config: &AbuseConfig) -> Router {
    apply_rate_limit(
        router,
        config.strict_rate_limit_rps,
        config.strict_rate_limit_burst,
        config.trust_proxy_headers,
    )
}

/// Apply the global concurrency limit with load-shedding for storage-DoS
/// isolation.
///
/// Excess requests beyond [`AbuseConfig::max_concurrency`] are shed with `503`
/// rather than queued unboundedly. Returns `router` unchanged when the limit is
/// `0` (disabled). See the module docs for what this does and does not guarantee.
pub fn apply_concurrency_load_shed(router: Router, config: &AbuseConfig) -> Router {
    if config.max_concurrency == 0 {
        return router;
    }
    router.layer(
        ServiceBuilder::new()
            .layer(HandleErrorLayer::new(|err: BoxError| async move {
                if err.is::<Overloaded>() {
                    ApiError::unavailable("server at capacity; please retry shortly")
                        .into_response()
                } else {
                    ApiError::internal("request processing error").into_response()
                }
            }))
            .load_shed()
            .concurrency_limit(config.max_concurrency),
    )
}

/// Wrap a fully-built application `router` with the **global** abuse controls.
///
/// Applied outer-to-inner: the default per-IP rate limit (rejects floods before
/// they consume a concurrency permit or a request body), then the concurrency +
/// load-shed limit, then an absolute body backstop sized to
/// [`AbuseConfig::upload_max_bytes`] that covers every route (including any future
/// or auth routes). Per-route tighter body caps and the strict rate-limit tier are
/// applied separately at route-construction time (see
/// [`crate::routes::hardened_router`]).
///
/// This is intended to be applied in the server binary *after* the router is
/// built, so it also protects routes added by other work packages.
pub fn apply_abuse_controls(router: Router, config: &AbuseConfig) -> Router {
    let router = router.layer(body_limit_layer(config.upload_max_bytes));
    let router = apply_concurrency_load_shed(router, config);
    apply_default_rate_limit(router, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative_and_enabled() {
        let config = AbuseConfig::default();
        assert!(config.rate_limit_rps > 0 && config.rate_limit_burst > 0);
        assert!(config.strict_rate_limit_rps > 0 && config.strict_rate_limit_burst > 0);
        // The strict tier is at least as tight as the default tier.
        assert!(config.strict_rate_limit_rps <= config.rate_limit_rps);
        // The upload cap must be able to hold at least a default-sized body.
        assert!(config.upload_max_bytes >= config.max_body_bytes);
        assert!(config.max_concurrency > 0);
        assert!(!config.trust_proxy_headers);
    }

    #[test]
    fn disabled_rate_limit_tier_is_a_no_op() {
        // A disabled tier must not attach a layer (so no ConnectInfo is needed).
        // We can only observe this indirectly: the function returns without panic
        // and the router remains usable. Building it is the assertion.
        let router: Router = Router::new();
        let _router = apply_rate_limit(router, 0, 100, false);
    }
}
