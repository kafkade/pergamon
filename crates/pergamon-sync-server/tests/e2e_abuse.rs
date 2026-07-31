// SPDX-License-Identifier: AGPL-3.0-only

//! Integration tests for the pre-auth abuse controls (WP-4, #195).
//!
//! These drive the hardened router (and the composable abuse layers) via
//! `tower::oneshot` — no real network. They prove:
//! - body-size caps reject oversized requests with `413` (per-route default cap
//!   and the larger upload cap), while normal-size requests pass;
//! - per-IP rate limiting rejects floods with `429`, lets callers under the limit
//!   through, and isolates distinct client IPs from one another;
//! - the storage-DoS concurrency/load-shed layer sheds excess with `503` rather
//!   than queueing unboundedly;
//! - `/health` is exempt from the strict tier;
//! - the default config does not regress a normal upload/push/pull round-trip.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pergamon_sync_server::abuse::apply_concurrency_load_shed;
use pergamon_sync_server::{
    AbuseConfig, AppState, SyncStore, apply_abuse_controls, build_router_hardened, ct_hash,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tower::ServiceExt;

const MIB: usize = 1024 * 1024;

/// Send a request against a clone of the router and return status + body bytes.
async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

/// Build a request carrying a `ConnectInfo<SocketAddr>` for the given client IP,
/// as `into_make_service_with_connect_info` would in production. The per-IP rate
/// limiter keys on this.
fn req_from_ip(method: &str, uri: &str, ip: &str, body: Body) -> Request<Body> {
    let addr = SocketAddr::new(ip.parse::<IpAddr>().unwrap(), 40000);
    Request::builder()
        .method(method)
        .uri(uri)
        .extension(ConnectInfo(addr))
        .body(body)
        .unwrap()
}

/// A config with all rate limits and the concurrency limit disabled, so a test
/// can exercise one control in isolation.
const fn quiet_config() -> AbuseConfig {
    AbuseConfig {
        rate_limit_rps: 0,
        rate_limit_burst: 0,
        strict_rate_limit_rps: 0,
        strict_rate_limit_burst: 0,
        max_body_bytes: 16 * MIB,
        upload_max_bytes: 64 * MIB,
        max_concurrency: 0,
        trust_proxy_headers: false,
    }
}

// ---------------------------------------------------------------------------
// Body-size caps
// ---------------------------------------------------------------------------

#[tokio::test]
async fn body_caps_reject_oversized_requests_and_pass_normal_ones() {
    let store = SyncStore::open_in_memory().unwrap();
    // Tiny caps so the test bodies are small: control routes 1 KiB, uploads 4 KiB.
    let cfg = AbuseConfig {
        max_body_bytes: 1024,
        upload_max_bytes: 4096,
        ..quiet_config()
    };
    let app = apply_abuse_controls(build_router_hardened(AppState::new(store), &cfg), &cfg);

    // (a) A control-route request (events, capped at max_body_bytes=1024) over the
    // default cap → 413.
    let oversized = vec![b'x'; 2048];
    let (status, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/v1/events")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, oversized.len())
            .body(Body::from(oversized))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "events over default cap"
    );

    // (b) A blob upload over the (larger) upload cap=4096 → 413.
    let too_big_blob = vec![b'y'; 8192];
    let hash = ct_hash(&too_big_blob);
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/acct-a/{hash}"))
            .header(header::CONTENT_LENGTH, too_big_blob.len())
            .body(Body::from(too_big_blob))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "blob over upload cap"
    );

    // (c) A normal-size blob upload (100 bytes, correct content hash) passes both
    // the upload cap and the global backstop → 201.
    let small_blob = vec![b'z'; 100];
    let hash = ct_hash(&small_blob);
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/acct-a/{hash}"))
            .header(header::CONTENT_LENGTH, small_blob.len())
            .body(Body::from(small_blob))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "normal-size upload passes");

    // (d) The two caps really differ: a 2000-byte body is rejected on the events
    // control route (default cap 1024) but accepted as a blob upload (cap 4096).
    let mid = vec![b'm'; 2000];
    let (status, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/v1/events")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, mid.len())
            .body(Body::from(mid.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "mid body over default cap"
    );

    let hash = ct_hash(&mid);
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/acct-a/{hash}"))
            .header(header::CONTENT_LENGTH, mid.len())
            .body(Body::from(mid))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mid body under upload cap");
}

// ---------------------------------------------------------------------------
// Per-IP rate limiting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_rate_limit_blocks_and_isolates_by_ip() {
    let store = SyncStore::open_in_memory().unwrap();
    // Strict tier: 1 token, refills 1/sec. Default tier disabled so only the
    // strict layer (on the events route) is in play.
    let cfg = AbuseConfig {
        strict_rate_limit_rps: 1,
        strict_rate_limit_burst: 1,
        ..quiet_config()
    };
    let app = build_router_hardened(AppState::new(store), &cfg);

    let uri = "/v1/events?account_id=acct-a&after=0";

    // IP1: first request consumes the only token → 200; the immediate second →
    // 429 (no token left within the same second).
    let (s1, _) = send(&app, req_from_ip("GET", uri, "10.0.0.1", Body::empty())).await;
    assert_eq!(s1, StatusCode::OK, "first request under the limit passes");
    let (s2, _) = send(&app, req_from_ip("GET", uri, "10.0.0.1", Body::empty())).await;
    assert_eq!(
        s2,
        StatusCode::TOO_MANY_REQUESTS,
        "second request is rate limited"
    );

    // IP2 is limited independently: it still has its own token → 200.
    let (s3, _) = send(&app, req_from_ip("GET", uri, "10.0.0.2", Body::empty())).await;
    assert_eq!(s3, StatusCode::OK, "a different IP is not affected");
}

#[tokio::test]
async fn default_rate_limit_covers_non_strict_routes() {
    let store = SyncStore::open_in_memory().unwrap();
    // Default tier: 1 token, refills 1/sec, applied globally by
    // `apply_abuse_controls`. Strict tier disabled.
    let cfg = AbuseConfig {
        rate_limit_rps: 1,
        rate_limit_burst: 1,
        ..quiet_config()
    };
    let app = apply_abuse_controls(build_router_hardened(AppState::new(store), &cfg), &cfg);

    // `/v1/devices/{account_id}` is a base (non-strict) route; the global default
    // tier still limits it.
    let uri = "/v1/devices/acct-a";
    let (s1, _) = send(&app, req_from_ip("GET", uri, "10.1.0.1", Body::empty())).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, _) = send(&app, req_from_ip("GET", uri, "10.1.0.1", Body::empty())).await;
    assert_eq!(s2, StatusCode::TOO_MANY_REQUESTS);
    // Isolation across IPs holds for the default tier too.
    let (s3, _) = send(&app, req_from_ip("GET", uri, "10.1.0.2", Body::empty())).await;
    assert_eq!(s3, StatusCode::OK);
}

#[tokio::test]
async fn health_is_exempt_from_the_strict_tier() {
    let store = SyncStore::open_in_memory().unwrap();
    // Strict tier extremely tight; default tier disabled. `/health` is not in any
    // strict route group, so it is never throttled by it.
    let cfg = AbuseConfig {
        strict_rate_limit_rps: 1,
        strict_rate_limit_burst: 1,
        ..quiet_config()
    };
    let app = build_router_hardened(AppState::new(store), &cfg);

    for _ in 0..5 {
        let (status, _) = send(
            &app,
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "/health must not be throttled");
    }
}

// ---------------------------------------------------------------------------
// Storage-DoS isolation: concurrency limit + load-shed
// ---------------------------------------------------------------------------

/// A gate that lets a test hold a request inside a handler deterministically.
struct Gate {
    entered: Notify,
    release: Notify,
}

async fn blocking_handler(State(gate): State<Arc<Gate>>) -> StatusCode {
    // Signal that we hold the concurrency permit, then block until released.
    gate.entered.notify_one();
    gate.release.notified().await;
    StatusCode::OK
}

#[tokio::test]
async fn concurrency_limit_sheds_excess_with_503() {
    let gate = Arc::new(Gate {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let cfg = AbuseConfig {
        max_concurrency: 1,
        ..quiet_config()
    };
    let app: Router = apply_concurrency_load_shed(
        Router::new()
            .route("/block", get(blocking_handler))
            .with_state(gate.clone()),
        &cfg,
    );

    // Request 1 acquires the single permit and blocks inside the handler.
    let app1 = app.clone();
    let inflight = tokio::spawn(async move {
        app1.oneshot(
            Request::builder()
                .uri("/block")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });

    // Wait until the handler is actually holding the permit.
    gate.entered.notified().await;

    // Request 2 arrives while the permit is held → shed with 503 (not queued).
    // A timeout guards against a regression that would queue instead of shed.
    let shed = tokio::time::timeout(
        Duration::from_secs(5),
        app.clone().oneshot(
            Request::builder()
                .uri("/block")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .expect("excess request must be shed promptly, not queued")
    .unwrap();
    assert_eq!(shed.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Release request 1; it completes normally with 200.
    gate.release.notify_one();
    let first = inflight.await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// No-regression: default config preserves a normal round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_config_preserves_upload_push_pull_round_trip() {
    let store = SyncStore::open_in_memory().unwrap();
    let cfg = AbuseConfig::default();
    let app = apply_abuse_controls(build_router_hardened(AppState::new(store), &cfg), &cfg);

    let account = "acct-regression";
    let device = "device-A";
    let ip = "10.2.0.1";

    // 1. Upload an opaque blob.
    let blob = b"opaque-ciphertext-bytes".to_vec();
    let blob_hash = ct_hash(&blob);
    let (status, _) = send(
        &app,
        req_from_ip(
            "PUT",
            &format!("/v1/blobs/{account}/{blob_hash}"),
            ip,
            Body::from(blob),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // 2. Push an event referencing the blob.
    let change_id = "change-1";
    let push = json!({
        "account_id": account,
        "events": [{
            "protocol_version": 1,
            "account_id": account,
            "device_id": device,
            "change_id": change_id,
            "key_epoch": 1,
            "blob_refs": [blob_hash],
            "ciphertext_b64": STANDARD.encode(b"event-ciphertext"),
        }]
    });
    let body = serde_json::to_vec(&push).unwrap();
    let mut req = req_from_ip("POST", "/v1/events", ip, Body::from(body));
    req.headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    let (status, resp_body) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(resp["high_water_seq"], 1);

    // 3. Pull it back.
    let (status, resp_body) = send(
        &app,
        req_from_ip(
            "GET",
            &format!("/v1/events?account_id={account}&after=0"),
            ip,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pull: Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(pull["events"][0]["change_id"], change_id);
}
