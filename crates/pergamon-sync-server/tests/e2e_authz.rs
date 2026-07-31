// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end authorization + tenant-isolation tests for WP-3c (issue #197).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! These drive the *real* multi-tenant router (`build_router_multitenant`) with
//! *real* OPAQUE register/login helpers (`pergamon-sync`, `auth` feature) and
//! *real* ADR-024 device keys (`pergamon-crypto`, a dev-dependency) to obtain
//! genuine bearer tokens, then prove the hard security boundary of #197:
//!
//! For **every** account-scoped content/relay route (path-, body-, and
//! query-scoped alike):
//! - no `Authorization` header → **401**;
//! - a valid token for account A hitting account A → **success** (never 401/403);
//! - a valid token for account A hitting account B → **403** (cross-tenant);
//! - a **revoked** token → **401**.
//!
//! Plus the open-surface guarantees: `/health` needs no token; a full
//! `/v1/auth/*` register→login round-trip still works in multi-tenant mode with
//! no pre-existing bearer; and — the byte-for-byte guard — **blind**-mode content
//! routes still work with NO token.
//!
//! The account the content routes key on is exactly the `account_id` returned by
//! OPAQUE `register/finish` (verified namespace: the token and the content plane
//! share the server-allocated handle).
//!
//! Helpers mirror `tests/e2e_tokens.rs`; they are duplicated here because Cargo
//! integration-test binaries cannot share code without a `common` module, and we
//! keep each suite self-contained (as `e2e_tokens.rs` already is).

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
    AbuseConfig, AppState, SyncStore, build_router, build_router_multitenant,
    build_router_multitenant_hardened, ct_hash,
};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use tower::ServiceExt;

const OPRF_KEY_ID: &str = "test-v1";

/// A fixed opaque blob used for the content-addressed blob routes. Its `ct_hash`
/// is deterministic, so a same-account `PUT` can present a matching hash.
const BLOB: &[u8] = b"opaque-ciphertext-blob-for-authz-tests";

// ---------------------------------------------------------------------------
// App + request builders
// ---------------------------------------------------------------------------

fn multitenant_app() -> (Router, AuthState) {
    let content = AppState::new(SyncStore::open_in_memory().unwrap());
    let store = AuthStore::open_in_memory().unwrap();
    let server_setup = ServerSetup::<PergamonCipherSuite>::new(&mut OsRng);
    let auth_state = AuthState::new(store, server_setup, OPRF_KEY_ID, ThrottleConfig::default());
    let app = build_router_multitenant(content, auth_state.clone());
    (app, auth_state)
}

fn blind_app() -> Router {
    build_router(AppState::new(SyncStore::open_in_memory().unwrap()))
}

/// The **hardened** multi-tenant router — the actual serve path in multi-tenant
/// mode (`main.rs`). It composes the WP-4 abuse layers (strict per-IP tier + body
/// caps on the events/blobs sub-routers) *under* the WP-3c auth layer, so this
/// exercises the layer-ordering interaction on the router that really ships. A
/// default [`AbuseConfig`] keeps normal-size requests below every cap/limit, so
/// the only rejections we observe come from the auth layer.
fn hardened_multitenant_app() -> (Router, AuthState) {
    let content = AppState::new(SyncStore::open_in_memory().unwrap());
    let store = AuthStore::open_in_memory().unwrap();
    let server_setup = ServerSetup::<PergamonCipherSuite>::new(&mut OsRng);
    let auth_state = AuthState::new(store, server_setup, OPRF_KEY_ID, ThrottleConfig::default());
    let app =
        build_router_multitenant_hardened(content, auth_state.clone(), &AbuseConfig::default());
    (app, auth_state)
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn b64d(value: &str) -> Vec<u8> {
    STANDARD.decode(value.as_bytes()).unwrap()
}

/// Start a request builder, attaching `Authorization: Bearer <token>` when a
/// bearer is provided.
fn builder(method: &str, uri: &str, bearer: Option<&str>) -> Builder {
    // Attach a peer-IP `ConnectInfo`, as `into_make_service_with_connect_info`
    // would in production. The hardened multi-tenant router's per-IP rate limiter
    // (WP-4) keys on this; the plain multi-tenant and blind routers simply ignore
    // the unused extension. A single fixed IP keeps every test's request count
    // well under the default strict burst (40).
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

/// A POST with a JSON body and an `Authorization: Bearer` header (for revoke).
fn post_auth(uri: &str, bearer: &str, body: &Value) -> Request<Body> {
    json_req("POST", uri, Some(bearer), body)
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
// The full set of account-scoped routes, built for one (account, device, bearer)
// ---------------------------------------------------------------------------

/// Every account-scoped content/relay route, as a labeled request bound to the
/// given `account`/`device` and (optionally) authenticated with `bearer`.
///
/// This is the exhaustive enforcement surface WP-3c gates: path-scoped (blobs,
/// devices, wraps, attestations, recovery), body-scoped (`/v1/events` push,
/// `/v1/blobs/probe`), and query-scoped (`/v1/events` pull).
fn account_routes(
    account: &str,
    device: &str,
    bearer: Option<&str>,
) -> Vec<(&'static str, Request<Body>)> {
    let ct = ct_hash(BLOB);
    vec![
        (
            "PUT /v1/blobs/<account>/<ct>",
            bytes_req("PUT", &format!("/v1/blobs/{account}/{ct}"), bearer, BLOB),
        ),
        (
            "GET /v1/blobs/<account>/<ct>",
            empty_req("GET", &format!("/v1/blobs/{account}/{ct}"), bearer),
        ),
        (
            "POST /v1/blobs/probe",
            json_req(
                "POST",
                "/v1/blobs/probe",
                bearer,
                &json!({ "account_id": account, "ct_hashes": [] }),
            ),
        ),
        (
            "POST /v1/events",
            json_req(
                "POST",
                "/v1/events",
                bearer,
                &json!({ "account_id": account, "events": [] }),
            ),
        ),
        (
            "GET /v1/events",
            empty_req("GET", &format!("/v1/events?account_id={account}"), bearer),
        ),
        (
            "GET /v1/devices/<account>",
            empty_req("GET", &format!("/v1/devices/{account}"), bearer),
        ),
        (
            "PUT /v1/devices/<account>/<device>",
            json_req(
                "PUT",
                &format!("/v1/devices/{account}/{device}"),
                bearer,
                &json!({ "record_b64": b64(b"device-record") }),
            ),
        ),
        (
            "GET /v1/devices/<account>/<device>",
            empty_req("GET", &format!("/v1/devices/{account}/{device}"), bearer),
        ),
        (
            "POST /v1/wraps/<account>/<device>",
            json_req(
                "POST",
                &format!("/v1/wraps/{account}/{device}"),
                bearer,
                &json!({ "bundle_b64": b64(b"wrap-bundle") }),
            ),
        ),
        (
            "GET /v1/wraps/<account>/<device>",
            empty_req("GET", &format!("/v1/wraps/{account}/{device}"), bearer),
        ),
        (
            "POST /v1/attestations/<account>",
            json_req(
                "POST",
                &format!("/v1/attestations/{account}"),
                bearer,
                &json!({ "attestation_b64": b64(b"attestation") }),
            ),
        ),
        (
            "GET /v1/attestations/<account>",
            empty_req("GET", &format!("/v1/attestations/{account}"), bearer),
        ),
        (
            "PUT /v1/recovery/<account>",
            json_req(
                "PUT",
                &format!("/v1/recovery/{account}"),
                bearer,
                &json!({ "blob_b64": b64(b"recovery-blob") }),
            ),
        ),
        (
            "GET /v1/recovery/<account>",
            empty_req("GET", &format!("/v1/recovery/{account}"), bearer),
        ),
    ]
}

// ---------------------------------------------------------------------------
// OPAQUE register / login / mint helpers (mirrors e2e_tokens.rs)
// ---------------------------------------------------------------------------

/// Drive the two-message OPAQUE registration through HTTP; return `account_id`.
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

/// A completed client-side login ready to drive `login/finish`.
struct FinishedLogin {
    login_id: String,
    finalization: Vec<u8>,
}

/// Drive OPAQUE `start` + client `finish` for `handle`.
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

/// Full happy path: register → login → mint. Returns `(account_id, token_bundle)`.
async fn register_login_mint(
    app: &Router,
    handle: &str,
    password: &[u8],
    device: &DeviceKeypairs,
) -> (String, Value) {
    let account_id = register(app, handle, password).await;
    let login = login_up_to_finish(app, handle, password).await;
    let pop = build_mint_pop(device, &login.login_id, &login.finalization);
    let (status, body) = send(
        app,
        post("/v1/auth/login/finish", &finish_body_with_pop(&login, &pop)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "login/finish with a valid PoP mints"
    );
    (account_id, parse(&body))
}

/// Register + mint and return `(account_id, device, access_token)`.
async fn account_with_token(app: &Router, handle: &str) -> (String, DeviceKeypairs, String) {
    let device = DeviceKeypairs::generate().unwrap();
    let (account_id, bundle) = register_login_mint(app, handle, b"correct horse", &device).await;
    let access = bundle["token"]["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    (account_id, device, access)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Every account-scoped route rejects a request with **no** `Authorization`
/// header with a uniform 401 — path-, body-, and query-scoped alike.
#[tokio::test]
async fn missing_bearer_is_401_on_every_account_route() {
    let (app, _auth) = multitenant_app();
    for (label, req) in account_routes("acct-anything", "dev-1", None) {
        let status = status_of(&app, req).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{label} without a bearer must be 401"
        );
    }
}

/// A valid token for account A can reach every one of A's routes: authorization
/// passes (never 401/403) and the seeded read-after-write routes return 200.
#[tokio::test]
async fn same_account_is_authorized_on_every_route() {
    let (app, _auth) = multitenant_app();
    let (account, device, access) = account_with_token(&app, "alice").await;
    let bearer = Some(access.as_str());
    let device_id = device.device_id();

    // First: none of A's own routes are rejected as unauthorized/forbidden.
    for (label, req) in account_routes(&account, device_id, bearer) {
        let status = status_of(&app, req).await;
        assert!(
            status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
            "{label} for the owning account must be authorized, got {status}",
        );
    }

    // Then: an explicit write→read sequence proves the routes are not merely
    // "authorized" but fully functional for the owner.
    let ct = ct_hash(BLOB);
    let expect = |app: &Router, req: Request<Body>, want: StatusCode, label: &'static str| {
        let app = app.clone();
        async move {
            assert_eq!(status_of(&app, req).await, want, "{label}");
        }
    };

    expect(
        &app,
        bytes_req("PUT", &format!("/v1/blobs/{account}/{ct}"), bearer, BLOB),
        StatusCode::CREATED,
        "PUT blob → 201",
    )
    .await;
    expect(
        &app,
        empty_req("GET", &format!("/v1/blobs/{account}/{ct}"), bearer),
        StatusCode::OK,
        "GET blob → 200",
    )
    .await;
    expect(
        &app,
        json_req(
            "POST",
            "/v1/blobs/probe",
            bearer,
            &json!({ "account_id": account, "ct_hashes": [] }),
        ),
        StatusCode::OK,
        "probe → 200",
    )
    .await;
    expect(
        &app,
        json_req(
            "POST",
            "/v1/events",
            bearer,
            &json!({ "account_id": account, "events": [] }),
        ),
        StatusCode::OK,
        "push events → 200",
    )
    .await;
    expect(
        &app,
        empty_req("GET", &format!("/v1/events?account_id={account}"), bearer),
        StatusCode::OK,
        "pull events → 200",
    )
    .await;
    expect(
        &app,
        json_req(
            "PUT",
            &format!("/v1/devices/{account}/{device_id}"),
            bearer,
            &json!({ "record_b64": b64(b"rec") }),
        ),
        StatusCode::CREATED,
        "PUT device → 201",
    )
    .await;
    expect(
        &app,
        empty_req("GET", &format!("/v1/devices/{account}/{device_id}"), bearer),
        StatusCode::OK,
        "GET device → 200",
    )
    .await;
    expect(
        &app,
        empty_req("GET", &format!("/v1/devices/{account}"), bearer),
        StatusCode::OK,
        "GET devices roster → 200",
    )
    .await;
    expect(
        &app,
        json_req(
            "POST",
            &format!("/v1/wraps/{account}/{device_id}"),
            bearer,
            &json!({ "bundle_b64": b64(b"bundle") }),
        ),
        StatusCode::OK,
        "POST wrap → 200",
    )
    .await;
    expect(
        &app,
        empty_req("GET", &format!("/v1/wraps/{account}/{device_id}"), bearer),
        StatusCode::OK,
        "GET wraps → 200",
    )
    .await;
    expect(
        &app,
        json_req(
            "POST",
            &format!("/v1/attestations/{account}"),
            bearer,
            &json!({ "attestation_b64": b64(b"att") }),
        ),
        StatusCode::OK,
        "POST attestation → 200",
    )
    .await;
    expect(
        &app,
        empty_req("GET", &format!("/v1/attestations/{account}"), bearer),
        StatusCode::OK,
        "GET attestations → 200",
    )
    .await;
    expect(
        &app,
        json_req(
            "PUT",
            &format!("/v1/recovery/{account}"),
            bearer,
            &json!({ "blob_b64": b64(b"blob") }),
        ),
        StatusCode::CREATED,
        "PUT recovery → 201",
    )
    .await;
    expect(
        &app,
        empty_req("GET", &format!("/v1/recovery/{account}"), bearer),
        StatusCode::OK,
        "GET recovery → 200",
    )
    .await;
}

/// A valid token for account A hitting **account B's** routes is denied with 403
/// on every route — the core cross-tenant isolation guarantee.
#[tokio::test]
async fn cross_tenant_is_403_on_every_account_route() {
    let (app, _auth) = multitenant_app();
    let (_account_a, _device_a, access_a) = account_with_token(&app, "alice").await;
    let (account_b, device_b, _access_b) = account_with_token(&app, "bob").await;

    // Alice's token, targeting Bob's account, on every route.
    for (label, req) in account_routes(&account_b, device_b.device_id(), Some(&access_a)) {
        let status = status_of(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: A→B must be 403");
    }
}

/// After a device self-revokes, its (now revoked) access token fails
/// authentication with 401 on every account route.
#[tokio::test]
async fn revoked_token_is_401_on_every_account_route() {
    let (app, auth) = multitenant_app();
    let (account, device, access) = account_with_token(&app, "alice").await;

    // Sanity: the token authenticates before revocation.
    assert!(auth.validate_token(&access).unwrap().is_some());

    // Self-revoke using the access token.
    let (status, body) = send(
        &app,
        post_auth(
            "/v1/auth/token/revoke",
            &access,
            &json!({ "device_id": device.device_id() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "self-revoke succeeds");
    assert!(parse(&body)["revoked"].as_u64().unwrap() >= 1);
    assert!(
        auth.validate_token(&access).unwrap().is_none(),
        "the revoked token must no longer validate",
    );

    // Every account route now rejects the revoked token as unauthenticated.
    for (label, req) in account_routes(&account, device.device_id(), Some(&access)) {
        let status = status_of(&app, req).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{label} with a revoked token must be 401"
        );
    }
}

/// `/health` stays open in multi-tenant mode: no token, still 200.
#[tokio::test]
async fn health_is_open_without_a_token() {
    let (app, _auth) = multitenant_app();
    let status = status_of(&app, empty_req("GET", "/health", None)).await;
    assert_eq!(status, StatusCode::OK, "/health must not require a bearer");
}

/// The `/v1/auth/*` control plane stays reachable without a pre-existing bearer:
/// a full register → login → mint round-trip works in multi-tenant mode.
#[tokio::test]
async fn auth_control_plane_round_trip_needs_no_bearer() {
    let (app, auth) = multitenant_app();
    let (account, device, access) = account_with_token(&app, "carol").await;
    // The minted token validates and resolves to the same (account, device).
    let who = auth
        .validate_token(&access)
        .unwrap()
        .expect("minted token validates");
    assert_eq!(who.account_id, account);
    assert_eq!(who.device_id, device.device_id());
}

/// The blind builder (`build_router`) is byte-for-byte unchanged: its content
/// routes still work with **no** `Authorization` header — the WP-3c layer is
/// never mounted there. (The existing blind suites — `e2e.rs`, `e2e_abuse.rs`,
/// convergence, crypto, onboarding — remain the primary proof; this is a direct
/// guard alongside them.)
#[tokio::test]
async fn blind_mode_content_routes_need_no_token() {
    let app = blind_app();
    let account = "self-hoster";
    let ct = ct_hash(BLOB);

    // Write then read a blob, push+pull events, probe — all token-less, all fine.
    assert_eq!(
        status_of(
            &app,
            bytes_req("PUT", &format!("/v1/blobs/{account}/{ct}"), None, BLOB)
        )
        .await,
        StatusCode::CREATED,
        "blind PUT blob works with no token",
    );
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/blobs/{account}/{ct}"), None)
        )
        .await,
        StatusCode::OK,
        "blind GET blob works with no token",
    );
    assert_eq!(
        status_of(
            &app,
            json_req(
                "POST",
                "/v1/events",
                None,
                &json!({ "account_id": account, "events": [] })
            ),
        )
        .await,
        StatusCode::OK,
        "blind push events works with no token",
    );
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/events?account_id={account}"), None)
        )
        .await,
        StatusCode::OK,
        "blind pull events works with no token",
    );
    assert_eq!(
        status_of(
            &app,
            json_req(
                "POST",
                "/v1/blobs/probe",
                None,
                &json!({ "account_id": account, "ct_hashes": [] })
            ),
        )
        .await,
        StatusCode::OK,
        "blind probe works with no token",
    );
}

/// The **hardened** multi-tenant builder (`build_router_multitenant_hardened`) —
/// the real multi-tenant serve path — still enforces WP-3c at **both**
/// enforcement points when the WP-4 abuse layers are present under the auth
/// layer. This guards against a layer-ordering regression silently dropping
/// enforcement on the router that actually ships.
#[tokio::test]
async fn hardened_multitenant_router_still_enforces_tenant_isolation() {
    let (app, _auth) = hardened_multitenant_app();
    let (account_a, device_a, access_a) = account_with_token(&app, "alice").await;
    let (account_b, device_b, _access_b) = account_with_token(&app, "bob").await;
    let ct = ct_hash(BLOB);

    // (a) Missing bearer → 401 (path route and body route alike).
    assert_eq!(
        status_of(
            &app,
            empty_req("GET", &format!("/v1/blobs/{account_a}/{ct}"), None),
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "hardened: path route without a bearer must be 401",
    );
    assert_eq!(
        status_of(
            &app,
            json_req(
                "POST",
                "/v1/events",
                None,
                &json!({ "account_id": account_a, "events": [] }),
            ),
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "hardened: body route without a bearer must be 401",
    );

    // (b) Cross-tenant A→B → 403 on a PATH-param route...
    assert_eq!(
        status_of(
            &app,
            empty_req(
                "GET",
                &format!("/v1/blobs/{account_b}/{ct}"),
                Some(&access_a)
            ),
        )
        .await,
        StatusCode::FORBIDDEN,
        "hardened: A→B on a path route must be 403",
    );
    // ...and on a BODY route...
    assert_eq!(
        status_of(
            &app,
            json_req(
                "POST",
                "/v1/events",
                Some(&access_a),
                &json!({ "account_id": account_b, "events": [] }),
            ),
        )
        .await,
        StatusCode::FORBIDDEN,
        "hardened: A→B on a body route must be 403",
    );
    // ...and on a QUERY route.
    assert_eq!(
        status_of(
            &app,
            empty_req(
                "GET",
                &format!("/v1/events?account_id={account_b}"),
                Some(&access_a),
            ),
        )
        .await,
        StatusCode::FORBIDDEN,
        "hardened: A→B on a query route must be 403",
    );

    // (c) Same-account A→A is authorized (never 401/403) at both enforcement
    //     points, proving the auth layer still fires under the abuse layers.
    let _ = device_b;
    for (label, req) in account_routes(&account_a, device_a.device_id(), Some(&access_a)) {
        let status = status_of(&app, req).await;
        assert!(
            status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
            "hardened: {label} for the owning account must be authorized, got {status}",
        );
    }
}
