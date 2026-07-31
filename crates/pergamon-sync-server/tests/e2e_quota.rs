// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end tests for per-tenant storage accounting + quotas (WP-3d, #198).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! These drive the *real* routers over `tower::ServiceExt::oneshot` and prove the
//! #198 acceptance surface:
//!
//! - **Accounting** — `GET /v1/usage/{account}` reports the exact summed
//!   ciphertext bytes/counts after blob PUTs + event pushes, and a re-uploaded
//!   blob / resent (deduped) batch does **not** inflate usage.
//! - **Enforcement** — with a small configured cap, an over-quota write is a
//!   **507 `QUOTA_EXCEEDED`**, the store is left unchanged (a partial batch never
//!   commits), reads still work while over quota, and an idempotent re-upload of
//!   an already-present blob still succeeds.
//! - **Isolation** — `GET /v1/usage/{B}` with A's token is **403** and a missing
//!   token is **401**, proving the endpoint inherits WP-3c tenant isolation via
//!   its `{account_id}` path param.
//! - **Unlimited default / blind mode** — with no quota configured, large writes
//!   succeed and blind-mode usage/writes work with no token (the no-regression
//!   guards).
//!
//! The multi-tenant helpers mirror `tests/e2e_authz.rs`; they are duplicated here
//! because Cargo integration-test binaries cannot share code without a `common`
//! module, and each suite is kept self-contained.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::too_many_lines)]

use std::net::{IpAddr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header, request::Builder};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use opaque_ke::ServerSetup;
use pergamon_crypto::device::DeviceKeypairs;
use pergamon_sync::auth::{ClientLoginFlow, ClientRegistrationFlow, DeviceMintPop, build_mint_pop};
use pergamon_sync_server::auth::store::AuthStore;
use pergamon_sync_server::auth::throttle::ThrottleConfig;
use pergamon_sync_server::auth::{AuthState, PergamonCipherSuite};
use pergamon_sync_server::{
    AppState, QuotaConfig, SyncStore, build_router, build_router_multitenant, ct_hash,
};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use tower::ServiceExt;

const OPRF_KEY_ID: &str = "test-v1";

// ---------------------------------------------------------------------------
// App + request builders
// ---------------------------------------------------------------------------

/// A blind router over an in-memory store with the given quota.
fn blind_app_with_quota(quota: QuotaConfig) -> Router {
    build_router(AppState::new(
        SyncStore::open_in_memory().unwrap().with_quota(quota),
    ))
}

/// A blind router with the default (unlimited) quota.
fn blind_app() -> Router {
    blind_app_with_quota(QuotaConfig::default())
}

/// A plain multi-tenant router (WP-3c auth layer, default unlimited quota).
fn multitenant_app() -> (Router, AuthState) {
    let content = AppState::new(SyncStore::open_in_memory().unwrap());
    let store = AuthStore::open_in_memory().unwrap();
    let server_setup = ServerSetup::<PergamonCipherSuite>::new(&mut OsRng);
    let auth_state = AuthState::new(store, server_setup, OPRF_KEY_ID, ThrottleConfig::default());
    let app = build_router_multitenant(content, auth_state.clone());
    (app, auth_state)
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn b64d(value: &str) -> Vec<u8> {
    STANDARD.decode(value.as_bytes()).unwrap()
}

/// Start a request builder, attaching a fixed peer-IP `ConnectInfo` (as
/// `into_make_service_with_connect_info` would) and an optional bearer.
fn builder(method: &str, uri: &str, bearer: Option<&str>) -> Builder {
    let addr = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 40000);
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .extension(ConnectInfo(addr));
    if let Some(token) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    b
}

fn json_req(method: &str, uri: &str, bearer: Option<&str>, body: &Value) -> Request<Body> {
    builder(method, uri, bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn empty_req(method: &str, uri: &str, bearer: Option<&str>) -> Request<Body> {
    builder(method, uri, bearer).body(Body::empty()).unwrap()
}

fn bytes_req(method: &str, uri: &str, bearer: Option<&str>, bytes: &[u8]) -> Request<Body> {
    builder(method, uri, bearer)
        .body(Body::from(bytes.to_vec()))
        .unwrap()
}

/// A plain POST with a JSON body and no bearer (for the `/v1/auth/*` flow).
fn post(uri: &str, body: &Value) -> Request<Body> {
    json_req("POST", uri, None, body)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

async fn status_of(app: &Router, req: Request<Body>) -> StatusCode {
    send(app, req).await.0
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

// ---------------------------------------------------------------------------
// Content-route request helpers (blind mode: no bearer)
// ---------------------------------------------------------------------------

/// A single event-envelope JSON with the given opaque ciphertext.
fn event_json(account: &str, device: &str, change_id: &str, ciphertext: &[u8]) -> Value {
    json!({
        "protocol_version": 1,
        "account_id": account,
        "device_id": device,
        "change_id": change_id,
        "key_epoch": 1,
        "blob_refs": [],
        "ciphertext_b64": b64(ciphertext),
        "sig_b64": "",
    })
}

/// `PUT /v1/blobs/{account}/{ct_hash(bytes)}` with the blob bytes as the body.
fn put_blob_req(account: &str, bytes: &[u8]) -> Request<Body> {
    let ct = ct_hash(bytes);
    bytes_req("PUT", &format!("/v1/blobs/{account}/{ct}"), None, bytes)
}

/// Fetch and parse `GET /v1/usage/{account}`.
async fn get_usage(app: &Router, account: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    let (status, body) = send(
        app,
        empty_req("GET", &format!("/v1/usage/{account}"), bearer),
    )
    .await;
    let value = if body.is_empty() {
        Value::Null
    } else {
        parse(&body)
    };
    (status, value)
}

// ---------------------------------------------------------------------------
// OPAQUE register / login / mint helpers (mirrors e2e_authz.rs)
// ---------------------------------------------------------------------------

async fn register(app: &Router, handle: &str, password: &[u8]) -> String {
    let (flow, request) = ClientRegistrationFlow::start(password).unwrap();
    let (status, body) = send(
        app,
        post(
            "/v1/auth/register/start",
            &json!({
                "identity_handle": handle,
                "registration_request_b64": b64(&request),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register/start should succeed");
    let response_b64 = parse(&body)["registration_response_b64"]
        .as_str()
        .unwrap()
        .to_string();

    let upload = flow.finish(password, &b64d(&response_b64)).unwrap();
    let (status, body) = send(
        app,
        post(
            "/v1/auth/register/finish",
            &json!({
                "identity_handle": handle,
                "registration_upload_b64": b64(&upload),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register/finish should succeed");
    parse(&body)["account_id"].as_str().unwrap().to_string()
}

struct FinishedLogin {
    login_id: String,
    finalization: Vec<u8>,
}

async fn login_up_to_finish(app: &Router, handle: &str, password: &[u8]) -> FinishedLogin {
    let (flow, ke1) = ClientLoginFlow::start(password).unwrap();
    let (status, body) = send(
        app,
        post(
            "/v1/auth/login/start",
            &json!({
                "identity_handle": handle,
                "credential_request_b64": b64(&ke1),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login/start should succeed");
    let v = parse(&body);
    let login_id = v["login_id"].as_str().unwrap().to_string();
    let ke2 = v["credential_response_b64"].as_str().unwrap().to_string();
    let finished = flow.finish(password, &b64d(&ke2)).unwrap();
    FinishedLogin {
        login_id,
        finalization: finished.finalization,
    }
}

fn finish_body_with_pop(login: &FinishedLogin, pop: &DeviceMintPop) -> Value {
    json!({
        "login_id": login.login_id,
        "credential_finalization_b64": b64(&login.finalization),
        "device_id": pop.device_id,
        "ed25519_pub_b64": pop.ed25519_pub_b64,
        "pop_signature_b64": pop.pop_signature_b64,
    })
}

/// Register + login + mint, returning `(account_id, access_token)`.
async fn account_with_token(app: &Router, handle: &str) -> (String, String) {
    let device = DeviceKeypairs::generate().unwrap();
    let password = b"correct horse";
    let account_id = register(app, handle, password).await;
    let login = login_up_to_finish(app, handle, password).await;
    let pop = build_mint_pop(&device, &login.login_id, &login.finalization);
    let (status, body) = send(
        app,
        post("/v1/auth/login/finish", &finish_body_with_pop(&login, &pop)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login/finish should mint a token");
    let access = parse(&body)["token"]["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    (account_id, access)
}

// ---------------------------------------------------------------------------
// Accounting
// ---------------------------------------------------------------------------

/// Usage reflects the exact summed ciphertext bytes/counts; a re-uploaded blob
/// and a resent (deduped) batch do not inflate it. Blind mode: no token needed.
#[tokio::test]
async fn usage_accounting_is_exact_and_dedup_correct() {
    let app = blind_app();
    let acct = "acct-blind";

    // Two distinct blobs (9 + 7 = 16 bytes, 2 objects).
    assert_eq!(
        status_of(&app, put_blob_req(acct, b"blob-aaaa")).await,
        StatusCode::CREATED
    );
    assert_eq!(
        status_of(&app, put_blob_req(acct, b"blob-bb")).await,
        StatusCode::CREATED
    );

    // Two events ("hello" = 5, "hi" = 2 -> 7 bytes, 2 objects).
    let push = json!({
        "account_id": acct,
        "events": [
            event_json(acct, "dev-1", "c1", b"hello"),
            event_json(acct, "dev-1", "c2", b"hi"),
        ],
    });
    assert_eq!(
        status_of(&app, json_req("POST", "/v1/events", None, &push)).await,
        StatusCode::OK
    );

    let (status, usage) = get_usage(&app, acct, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "blind usage is open without a token"
    );
    assert_eq!(usage["blob_bytes"], 16);
    assert_eq!(usage["blob_count"], 2);
    assert_eq!(usage["event_bytes"], 7);
    assert_eq!(usage["event_count"], 2);
    assert_eq!(usage["total_bytes"], 23);
    assert_eq!(usage["total_objects"], 4);
    assert_eq!(usage["max_account_bytes"], 0);
    assert_eq!(usage["max_account_objects"], 0);
    assert_eq!(usage["over_quota"], false);

    // Re-upload a blob and resend the identical batch: both dedupe, so usage is
    // unchanged.
    assert_eq!(
        status_of(&app, put_blob_req(acct, b"blob-aaaa")).await,
        StatusCode::CREATED
    );
    assert_eq!(
        status_of(&app, json_req("POST", "/v1/events", None, &push)).await,
        StatusCode::OK
    );
    let (_, usage2) = get_usage(&app, acct, None).await;
    assert_eq!(usage2["total_bytes"], 23);
    assert_eq!(usage2["total_objects"], 4);
}

// ---------------------------------------------------------------------------
// Enforcement
// ---------------------------------------------------------------------------

/// An over-byte-quota blob PUT is 507 `QUOTA_EXCEEDED`; the store is unchanged;
/// reads still work; and an idempotent re-upload of a present blob still
/// succeeds while at/over quota.
#[tokio::test]
async fn over_byte_quota_blob_put_is_507_reads_still_work() {
    let app = blind_app_with_quota(QuotaConfig {
        max_account_bytes: 9,
        max_account_objects: 0,
    });
    let acct = "acct-q";
    let blob_a = b"012345678"; // 9 bytes: exactly the cap.

    assert_eq!(
        status_of(&app, put_blob_req(acct, blob_a)).await,
        StatusCode::CREATED
    );

    // A second distinct blob would exceed the cap.
    let (status, body) = send(&app, put_blob_req(acct, b"xyz")).await;
    assert_eq!(
        status,
        StatusCode::INSUFFICIENT_STORAGE,
        "over quota -> 507"
    );
    assert_eq!(parse(&body)["code"], "QUOTA_EXCEEDED");

    // The rejected write left the store unchanged.
    let (_, usage) = get_usage(&app, acct, None).await;
    assert_eq!(usage["total_bytes"], 9);
    assert_eq!(usage["total_objects"], 1);
    assert_eq!(usage["over_quota"], false);

    // Reads still work while at quota: blob GET, probe, and usage.
    let ct = ct_hash(blob_a);
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/blobs/{acct}/{ct}"), None)
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(
            &app,
            json_req(
                "POST",
                "/v1/blobs/probe",
                None,
                &json!({ "account_id": acct, "ct_hashes": [ct] }),
            ),
        )
        .await,
        StatusCode::OK
    );

    // An idempotent re-upload of the already-present blob is still allowed.
    assert_eq!(
        status_of(&app, put_blob_req(acct, blob_a)).await,
        StatusCode::CREATED
    );
}

/// An over-object-quota blob PUT is 507 `QUOTA_EXCEEDED`.
#[tokio::test]
async fn over_object_quota_blob_put_is_507() {
    let app = blind_app_with_quota(QuotaConfig {
        max_account_bytes: 0,
        max_account_objects: 1,
    });
    let acct = "acct-obj";
    assert_eq!(
        status_of(&app, put_blob_req(acct, b"one")).await,
        StatusCode::CREATED
    );
    let (status, body) = send(&app, put_blob_req(acct, b"two")).await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(parse(&body)["code"], "QUOTA_EXCEEDED");
    let (_, usage) = get_usage(&app, acct, None).await;
    assert_eq!(usage["total_objects"], 1);
}

/// An event push that would exceed the byte cap is 507 and the whole batch is
/// rolled back (partial batch never commits); reads still work afterwards.
#[tokio::test]
async fn over_quota_event_push_is_507_and_batch_not_committed() {
    let app = blind_app_with_quota(QuotaConfig {
        max_account_bytes: 5,
        max_account_objects: 0,
    });
    let acct = "acct-ev";
    let push = json!({
        "account_id": acct,
        "events": [
            event_json(acct, "dev-1", "c1", b"0123456789"),
            event_json(acct, "dev-1", "c2", b"abcdefghij"),
        ],
    });
    let (status, body) = send(&app, json_req("POST", "/v1/events", None, &push)).await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(parse(&body)["code"], "QUOTA_EXCEEDED");

    // Nothing committed.
    let (_, usage) = get_usage(&app, acct, None).await;
    assert_eq!(usage["event_count"], 0);
    assert_eq!(usage["total_bytes"], 0);

    // A pull still works (read allowed while over/at quota).
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/events?account_id={acct}"), None)
        )
        .await,
        StatusCode::OK
    );
}

/// With the default (unlimited) quota, large writes succeed.
#[tokio::test]
async fn unlimited_default_allows_large_writes() {
    let app = blind_app();
    let acct = "acct-big";
    let big = vec![7_u8; 200_000];
    assert_eq!(
        status_of(&app, put_blob_req(acct, &big)).await,
        StatusCode::CREATED
    );
    let (status, usage) = get_usage(&app, acct, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(usage["blob_bytes"], 200_000);
    assert_eq!(usage["over_quota"], false);
}

// ---------------------------------------------------------------------------
// Isolation (multi-tenant) — proves WP-3c gating of the usage route
// ---------------------------------------------------------------------------

/// `GET /v1/usage/{account}` inherits WP-3c tenant isolation: A→A is 200, A→B is
/// 403, and a missing token is 401.
#[tokio::test]
async fn usage_route_enforces_tenant_isolation() {
    let (app, _auth) = multitenant_app();
    let (account_a, token_a) = account_with_token(&app, "alice").await;
    let (account_b, _token_b) = account_with_token(&app, "bob").await;

    // A reading its own usage: authorized.
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/usage/{account_a}"), Some(&token_a)),
        )
        .await,
        StatusCode::OK,
        "A reading its own usage must be authorized",
    );

    // A reading B's usage: cross-tenant, forbidden.
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/usage/{account_b}"), Some(&token_a)),
        )
        .await,
        StatusCode::FORBIDDEN,
        "A reading B's usage must be 403",
    );

    // No bearer: unauthenticated.
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/usage/{account_a}"), None)
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "usage without a token must be 401 in multi-tenant mode",
    );
}
