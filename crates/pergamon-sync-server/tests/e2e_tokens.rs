// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end interop tests for the per-device token control plane
//! (WP-3b, issue #192).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! These drive the *real* server router (`build_router_multitenant`) with the
//! *real* Apache-2.0 client helpers from `pergamon-sync` (the `auth` feature) and
//! *real* ADR-024 device keys from `pergamon-crypto` (a dev-dependency), proving
//! the acceptance criteria of #192:
//!
//! 1. issuance is tied to a device-key proof-of-possession: a correct signature
//!    mints a token bound to `(account_id, device_id, ed25519_pub)`; a
//!    wrong/absent signature or a `device_id` that does not match the key is
//!    rejected; and a mint-PoP for one login cannot be replayed into another.
//! 2. a minted token is scoped to exactly one `account_id` (tenant isolation).
//! 3. refresh with a fresh `PoP` works.
//! 4. revocation: a revoked device fails `validate_token` and refresh.
//! 5. `blind` mode does not mount the token routes (404); `multitenant` does.
//! 6. full interop: register → login → mint → refresh, and revoked → rejected.
//!
//! The `PoP` binding builders are duplicated across the AGPL server
//! (`auth::token`) and the Apache client (`pergamon_sync::auth`) to respect the
//! license boundary; these tests passing is the guardrail that they match.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::too_many_lines)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use opaque_ke::ServerSetup;
use pergamon_crypto::device::DeviceKeypairs;
use pergamon_sync::auth::{
    ClientLoginFlow, ClientRegistrationFlow, DeviceMintPop, build_mint_pop, build_refresh_request,
    fresh_nonce,
};
use pergamon_sync_server::auth::store::AuthStore;
use pergamon_sync_server::auth::throttle::ThrottleConfig;
use pergamon_sync_server::auth::{AuthState, PergamonCipherSuite};
use pergamon_sync_server::{AppState, SyncStore, build_router, build_router_multitenant};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use tower::ServiceExt;

const OPRF_KEY_ID: &str = "test-v1";

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

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn b64d(value: &str) -> Vec<u8> {
    STANDARD.decode(value.as_bytes()).unwrap()
}

fn post(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// A POST request with a JSON body and an `Authorization: Bearer` header.
fn post_auth(uri: &str, bearer: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn send_json(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// Drive the two-message OPAQUE registration through HTTP; return `account_id`.
async fn register(app: &Router, handle: &str, password: &[u8]) -> String {
    let (flow, request) = ClientRegistrationFlow::start(password).unwrap();
    let (status, body) = send_json(
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
    let (status, body) = send_json(
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

/// Run `login/start` and return `(login_id, credential_response_b64)`.
async fn login_start(app: &Router, handle: &str, ke1: &[u8]) -> (String, String) {
    let (status, body) = send_json(
        app,
        post(
            "/v1/auth/login/start",
            &json!({
                "identity_handle": handle,
                "credential_request_b64": b64(ke1),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login/start should succeed");
    let v = parse(&body);
    (
        v["login_id"].as_str().unwrap().to_string(),
        v["credential_response_b64"].as_str().unwrap().to_string(),
    )
}

/// A completed client-side login ready to drive `login/finish`.
struct FinishedLogin {
    login_id: String,
    finalization: Vec<u8>,
}

/// Drive OPAQUE `start` + client `finish` for `handle`, returning the pieces the
/// mint-PoP is bound to (`login_id` + the KE3 finalization bytes).
async fn login_up_to_finish(app: &Router, handle: &str, password: &[u8]) -> FinishedLogin {
    let (flow, ke1) = ClientLoginFlow::start(password).unwrap();
    let (login_id, ke2) = login_start(app, handle, &ke1).await;
    let finished = flow.finish(password, &b64d(&ke2)).unwrap();
    FinishedLogin {
        login_id,
        finalization: finished.finalization,
    }
}

/// Body for `login/finish` carrying a device `PoP` built from `pop`.
fn finish_body_with_pop(login: &FinishedLogin, pop: &DeviceMintPop) -> Value {
    json!({
        "login_id": login.login_id,
        "credential_finalization_b64": b64(&login.finalization),
        "device_id": pop.device_id,
        "ed25519_pub_b64": pop.ed25519_pub_b64,
        "pop_signature_b64": pop.pop_signature_b64,
    })
}

/// Full happy-path: register → login → mint. Returns the parsed token bundle.
async fn register_login_mint(
    app: &Router,
    handle: &str,
    password: &[u8],
    device: &DeviceKeypairs,
) -> (String, Value) {
    let account_id = register(app, handle, password).await;
    let login = login_up_to_finish(app, handle, password).await;
    let pop = build_mint_pop(device, &login.login_id, &login.finalization);
    let (status, body) = send_json(
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

/// Test 1: a valid `PoP` mints a token bundle bound to the login's account, the
/// requesting device, and a validating access token (proves issuance is tied to
/// device-key proof-of-possession).
#[tokio::test]
async fn valid_pop_mints_bound_token_bundle() {
    let (app, auth) = multitenant_app();
    let device = DeviceKeypairs::generate().unwrap();
    let (account_id, v) = register_login_mint(&app, "alice", b"correct horse", &device).await;

    assert_eq!(v["authenticated"], json!(true));
    let token = &v["token"];
    assert!(token.is_object(), "a valid PoP must mint a token bundle");
    assert_eq!(token["account_id"].as_str().unwrap(), account_id);
    assert_eq!(token["device_id"].as_str().unwrap(), device.device_id());

    // The minted access token validates against the reusable primitive and
    // resolves to the same (account_id, device_id).
    let access = token["access_token"].as_str().unwrap();
    let who = auth
        .validate_token(access)
        .unwrap()
        .expect("access validates");
    assert_eq!(who.account_id, account_id);
    assert_eq!(who.device_id, device.device_id());
}

/// Test 1b: a login carrying **no** `PoP` authenticates exactly as in WP-3a and
/// mints no token (the response is the WP-3a shape — `token` omitted).
#[tokio::test]
async fn login_without_pop_mints_no_token() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"pw").await;
    let login = login_up_to_finish(&app, "alice", b"pw").await;
    let (status, body) = send_json(
        &app,
        post(
            "/v1/auth/login/finish",
            &json!({
                "login_id": login.login_id,
                "credential_finalization_b64": b64(&login.finalization),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse(&body);
    assert_eq!(v["authenticated"], json!(true));
    assert!(v.get("token").is_none() || v["token"].is_null());
}

/// Test 1c: a forged signature is rejected (401) and mints nothing.
#[tokio::test]
async fn wrong_signature_is_rejected() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"pw").await;
    let device = DeviceKeypairs::generate().unwrap();
    let login = login_up_to_finish(&app, "alice", b"pw").await;
    let mut pop = build_mint_pop(&device, &login.login_id, &login.finalization);
    // Corrupt the signature (still valid base64 of 64 bytes).
    pop.pop_signature_b64 = b64(&[0u8; 64]);
    let (status, _) = send_json(
        &app,
        post("/v1/auth/login/finish", &finish_body_with_pop(&login, &pop)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Test 1d: a `device_id` that does not match the presented key is rejected.
#[tokio::test]
async fn device_id_not_matching_key_is_rejected() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"pw").await;
    let device = DeviceKeypairs::generate().unwrap();
    let other = DeviceKeypairs::generate().unwrap();
    let login = login_up_to_finish(&app, "alice", b"pw").await;
    let mut pop = build_mint_pop(&device, &login.login_id, &login.finalization);
    // Present a different device_id than the one bound to `device`'s key.
    pop.device_id = other.device_id().to_string();
    let (status, _) = send_json(
        &app,
        post("/v1/auth/login/finish", &finish_body_with_pop(&login, &pop)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Test 1e: a partial `PoP` (some but not all three fields) is a malformed request.
#[tokio::test]
async fn partial_pop_is_bad_request() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"pw").await;
    let device = DeviceKeypairs::generate().unwrap();
    let login = login_up_to_finish(&app, "alice", b"pw").await;
    let (status, _) = send_json(
        &app,
        post(
            "/v1/auth/login/finish",
            &json!({
                "login_id": login.login_id,
                "credential_finalization_b64": b64(&login.finalization),
                "device_id": device.device_id(),
                // ed25519_pub_b64 and pop_signature_b64 deliberately omitted.
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Test 1f: a mint-PoP captured from one login cannot be replayed into a
/// different login (single-use binding to the server-issued `login_id` + the KE3
/// transcript).
#[tokio::test]
async fn mint_pop_is_not_replayable_across_logins() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"pw").await;
    let device = DeviceKeypairs::generate().unwrap();

    // Login A: build (but do not necessarily submit) a valid PoP.
    let login_a = login_up_to_finish(&app, "alice", b"pw").await;
    let pop_a = build_mint_pop(&device, &login_a.login_id, &login_a.finalization);

    // Login B: a fresh, independent login for the same identity.
    let login_b = login_up_to_finish(&app, "alice", b"pw").await;

    // Submit login B's finish with login A's PoP signature (and B's transcript).
    // The signature was made over A's login_id + A's KE3, so it fails for B.
    let replay = json!({
        "login_id": login_b.login_id,
        "credential_finalization_b64": b64(&login_b.finalization),
        "device_id": pop_a.device_id,
        "ed25519_pub_b64": pop_a.ed25519_pub_b64,
        "pop_signature_b64": pop_a.pop_signature_b64,
    });
    let (status, _) = send_json(&app, post("/v1/auth/login/finish", &replay)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Test 2: minted tokens are scoped to exactly one `account_id`, and one tenant
/// cannot revoke another tenant's device.
#[tokio::test]
async fn tokens_are_scoped_to_one_account() {
    let (app, auth) = multitenant_app();
    let alice_dev = DeviceKeypairs::generate().unwrap();
    let bob_dev = DeviceKeypairs::generate().unwrap();

    let (alice_acct, alice_tok) = register_login_mint(&app, "alice", b"alice-pw", &alice_dev).await;
    let (bob_acct, bob_tok) = register_login_mint(&app, "bob", b"bob-pw", &bob_dev).await;

    assert_ne!(alice_acct, bob_acct, "distinct accounts");
    assert_eq!(
        alice_tok["token"]["account_id"].as_str().unwrap(),
        alice_acct
    );
    assert_eq!(bob_tok["token"]["account_id"].as_str().unwrap(), bob_acct);

    let alice_access = alice_tok["token"]["access_token"].as_str().unwrap();
    let bob_access = bob_tok["token"]["access_token"].as_str().unwrap();

    // Alice, using her own access token, tries to revoke *Bob's* device. The
    // revoke is scoped to the caller's account, so it revokes nothing and Bob's
    // token keeps validating.
    let (status, body) = send_json(
        &app,
        post_auth(
            "/v1/auth/token/revoke",
            alice_access,
            &json!({ "device_id": bob_dev.device_id() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        parse(&body)["revoked"].as_u64().unwrap(),
        0,
        "cross-tenant revoke is a no-op"
    );
    assert!(
        auth.validate_token(bob_access).unwrap().is_some(),
        "Bob's token must survive Alice's cross-tenant revoke attempt"
    );
}

/// Test 3: refresh with a fresh device `PoP` mints a new, validating access token
/// and **rotates** the refresh token (returns a fresh, different one).
#[tokio::test]
async fn refresh_with_fresh_pop_mints_new_access_token() {
    let (app, auth) = multitenant_app();
    let device = DeviceKeypairs::generate().unwrap();
    let (account_id, v) = register_login_mint(&app, "alice", b"pw", &device).await;
    let refresh_token = v["token"]["refresh_token"].as_str().unwrap().to_string();

    let nonce = fresh_nonce();
    let req = build_refresh_request(&device, &refresh_token, &nonce);
    let (status, body) = send_json(
        &app,
        post(
            "/v1/auth/token/refresh",
            &serde_json::to_value(&req).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "refresh with a fresh PoP works");
    let new_access = parse(&body)["access_token"].as_str().unwrap().to_string();
    let new_refresh = parse(&body)["refresh_token"].as_str().unwrap().to_string();
    assert!(
        !new_refresh.is_empty(),
        "refresh is rotated: a fresh refresh token is returned"
    );
    assert_ne!(
        new_refresh, refresh_token,
        "the rotated refresh token differs from the one just presented"
    );
    let who = auth
        .validate_token(&new_access)
        .unwrap()
        .expect("new access validates");
    assert_eq!(who.account_id, account_id);
    assert_eq!(who.device_id, device.device_id());
}

/// Test 3a: the refresh token is **single-use** — after a successful refresh the
/// presented (old) refresh token is rejected, while the rotated one works.
#[tokio::test]
async fn refresh_token_is_single_use_after_rotation() {
    let (app, _auth) = multitenant_app();
    let device = DeviceKeypairs::generate().unwrap();
    let (_acct, v) = register_login_mint(&app, "alice", b"pw", &device).await;
    let old_refresh = v["token"]["refresh_token"].as_str().unwrap().to_string();

    // First refresh: rotates old_refresh -> new_refresh.
    let req = build_refresh_request(&device, &old_refresh, &fresh_nonce());
    let (status, body) = send_json(
        &app,
        post(
            "/v1/auth/token/refresh",
            &serde_json::to_value(&req).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let new_refresh = parse(&body)["refresh_token"].as_str().unwrap().to_string();

    // Replaying the OLD (now revoked) refresh token must be rejected.
    let replay = build_refresh_request(&device, &old_refresh, &fresh_nonce());
    let (status, _) = send_json(
        &app,
        post(
            "/v1/auth/token/refresh",
            &serde_json::to_value(&replay).unwrap(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a rotated (single-use) refresh token cannot be replayed"
    );

    // The NEW refresh token still works.
    let again = build_refresh_request(&device, &new_refresh, &fresh_nonce());
    let (status, _) = send_json(
        &app,
        post(
            "/v1/auth/token/refresh",
            &serde_json::to_value(&again).unwrap(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the rotated refresh token is valid for the next exchange"
    );
}

/// Test 3b: refresh signed by the *wrong* device key is rejected.
#[tokio::test]
async fn refresh_with_wrong_device_is_rejected() {
    let (app, _auth) = multitenant_app();
    let device = DeviceKeypairs::generate().unwrap();
    let attacker = DeviceKeypairs::generate().unwrap();
    let (_acct, v) = register_login_mint(&app, "alice", b"pw", &device).await;
    let refresh_token = v["token"]["refresh_token"].as_str().unwrap().to_string();

    // The attacker holds the (bearer) refresh string but not the bound key.
    let nonce = fresh_nonce();
    let req = build_refresh_request(&attacker, &refresh_token, &nonce);
    let (status, _) = send_json(
        &app,
        post(
            "/v1/auth/token/refresh",
            &serde_json::to_value(&req).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Test 4 + 6: revocation. After revoking the device, its access token fails
/// `validate_token` and its refresh token can no longer mint access tokens.
#[tokio::test]
async fn revoked_device_fails_validate_and_refresh() {
    let (app, auth) = multitenant_app();
    let device = DeviceKeypairs::generate().unwrap();
    let (_account_id, v) = register_login_mint(&app, "alice", b"pw", &device).await;
    let access = v["token"]["access_token"].as_str().unwrap().to_string();
    let refresh_token = v["token"]["refresh_token"].as_str().unwrap().to_string();

    // Pre-revocation both work.
    assert!(auth.validate_token(&access).unwrap().is_some());

    // Self-revoke using the access token.
    let (status, body) = send_json(
        &app,
        post_auth(
            "/v1/auth/token/revoke",
            &access,
            &json!({ "device_id": device.device_id() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        parse(&body)["revoked"].as_u64().unwrap() >= 1,
        "access + refresh revoked"
    );

    // The access token now fails validation.
    assert!(
        auth.validate_token(&access).unwrap().is_none(),
        "a revoked device's access token must fail validate_token"
    );

    // ...and it can no longer authenticate a revoke call.
    let (status, _) = send_json(
        &app,
        post_auth(
            "/v1/auth/token/revoke",
            &access,
            &json!({ "device_id": device.device_id() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The refresh token can no longer mint an access token.
    let nonce = fresh_nonce();
    let req = build_refresh_request(&device, &refresh_token, &nonce);
    let (status, _) = send_json(
        &app,
        post(
            "/v1/auth/token/refresh",
            &serde_json::to_value(&req).unwrap(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "refresh after revocation must be rejected"
    );
}

/// Test 5: blind mode does not mount the token routes (`404`); multi-tenant mode
/// does (a well-formed-but-invalid request is handled, not `404`).
#[tokio::test]
async fn blind_mode_does_not_mount_token_routes() {
    let blind = blind_app();
    let (status, _) = send_json(
        &blind,
        post(
            "/v1/auth/token/refresh",
            &json!({
                "refresh_token": "x.y",
                "device_id": "d",
                "ed25519_pub_b64": b64(&[0u8; 32]),
                "nonce_b64": b64(&[0u8; 16]),
                "pop_signature_b64": b64(&[0u8; 64]),
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "blind mode must not expose tokens"
    );

    // Multi-tenant mode mounts them: an invalid refresh is a 401, not a 404.
    let (mt, _auth) = multitenant_app();
    let device = DeviceKeypairs::generate().unwrap();
    let nonce = fresh_nonce();
    let req = build_refresh_request(&device, "unknown-id.c2VjcmV0", &nonce);
    let (status, _) = send_json(
        &mt,
        post(
            "/v1/auth/token/refresh",
            &serde_json::to_value(&req).unwrap(),
        ),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "multi-tenant mounts the token plane"
    );
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
