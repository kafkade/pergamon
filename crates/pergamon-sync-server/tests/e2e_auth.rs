// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end integration tests for the OPAQUE server-auth control plane
//! (WP-3a, issue #189).
//!
//! # ⚠️ NOT YET EXTERNALLY SECURITY-REVIEWED — DO NOT DEPLOY ⚠️
//!
//! These tests drive the *real* server router (`build_router_multitenant`) with
//! the *real* Apache-2.0 client helpers from `pergamon-sync` (the `auth`
//! feature), proving the cross-crate OPAQUE round trip and the security
//! properties the design (`docs/design/hosted-auth-control-plane.md`) requires:
//!
//! 1. register → login round-trip succeeds and authenticates.
//! 2. login with a wrong password fails.
//! 3. an unknown identity is indistinguishable from a wrong password — no
//!    account-existence oracle (§1.6): identical KE2 shape, identical client
//!    failure, and identical uniform `401` bodies from `login/finish`.
//! 4. the stored `accounts` row is the OPAQUE verifier **only** — it contains no
//!    password/password-equivalent bytes (§1.5).
//! 5. the OPRF server secret (`ServerSetup`) lives outside the verifier rows
//!    (§1.8).
//! 6. per-identity throttling: after enough failures one identity is locked out
//!    with a uniform `429` (§1.7).
//! 7. `blind` mode does not mount the auth routes (404); `multitenant` does.
//!
//! The two definitions of `PergamonCipherSuite` (server-side, AGPL; client-side,
//! Apache) are deliberately duplicated to respect the license boundary; test #1
//! passing is the guardrail that they have not drifted.

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
use pergamon_sync::auth::{ClientLoginFlow, ClientRegistrationFlow};
use pergamon_sync_server::auth::store::AuthStore;
use pergamon_sync_server::auth::throttle::ThrottleConfig;
use pergamon_sync_server::auth::{AuthState, PergamonCipherSuite};
use pergamon_sync_server::{AppState, SyncStore, build_router, build_router_multitenant};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use tower::ServiceExt;

const OPRF_KEY_ID: &str = "test-v1";

/// Build a multi-tenant router plus a retained clone of the [`AuthState`] so a
/// test can both drive HTTP requests and directly inspect the auth store /
/// server secret afterwards.
fn multitenant_app() -> (Router, AuthState) {
    let content = AppState::new(SyncStore::open_in_memory().unwrap());
    let store = AuthStore::open_in_memory().unwrap();
    let server_setup = ServerSetup::<PergamonCipherSuite>::new(&mut OsRng);
    let auth_state = AuthState::new(store, server_setup, OPRF_KEY_ID, ThrottleConfig::default());
    let app = build_router_multitenant(content, auth_state.clone());
    (app, auth_state)
}

/// Build the blind router (no auth plane) for the mode-gating test.
fn blind_app() -> Router {
    build_router(AppState::new(SyncStore::open_in_memory().unwrap()))
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn b64d(value: &str) -> Vec<u8> {
    STANDARD.decode(value.as_bytes()).unwrap()
}

/// A POST request with a JSON body.
fn post(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// Send a request against a clone of the router; return status + parsed body.
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

/// `true` if `needle` appears as a contiguous subsequence of `haystack`.
fn contains_subseq(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return needle.is_empty();
    }
    haystack.windows(needle.len()).any(|w| w == needle)
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

/// Perform `login/start` and return `(status, login_id, credential_response_b64)`.
async fn login_start(
    app: &Router,
    handle: &str,
    ke1: &[u8],
) -> (StatusCode, Option<(String, String)>) {
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
    if status == StatusCode::OK {
        let v = parse(&body);
        let login_id = v["login_id"].as_str().unwrap().to_string();
        let ke2 = v["credential_response_b64"].as_str().unwrap().to_string();
        (status, Some((login_id, ke2)))
    } else {
        (status, None)
    }
}

/// Perform `login/finish` and return `(status, body_bytes)`.
async fn login_finish(app: &Router, login_id: &str, finalization: &[u8]) -> (StatusCode, Vec<u8>) {
    send_json(
        app,
        post(
            "/v1/auth/login/finish",
            &json!({
                "login_id": login_id,
                "credential_finalization_b64": b64(finalization),
            }),
        ),
    )
    .await
}

/// Produce a well-formed but *wrong* KE3 for `handle`: a real finalization built
/// against one `login/start`'s KE2, later submitted under a *different*
/// `login_id`. The MAC binds the transcript, so the server rejects it with the
/// uniform `401` — exactly what a wrong-password attempt yields on the wire.
async fn stale_finalization(app: &Router, handle: &str, password: &[u8]) -> Vec<u8> {
    let (flow, ke1) = ClientLoginFlow::start(password).unwrap();
    let (status, started) = login_start(app, handle, &ke1).await;
    assert_eq!(status, StatusCode::OK);
    let (_login_id, ke2) = started.unwrap();
    let finished = flow.finish(password, &b64d(&ke2)).unwrap();
    finished.finalization
}

/// Test 1: a full register → login round-trip succeeds and authenticates, and
/// the login returns the same opaque `account_id` allocated at registration.
#[tokio::test]
async fn register_then_login_round_trip_succeeds() {
    let (app, _auth) = multitenant_app();
    let password = b"correct horse battery staple";
    let reg_account_id = register(&app, "alice", password).await;

    let (flow, ke1) = ClientLoginFlow::start(password).unwrap();
    let (status, started) = login_start(&app, "alice", &ke1).await;
    assert_eq!(status, StatusCode::OK);
    let (login_id, ke2) = started.unwrap();

    let finished = flow.finish(password, &b64d(&ke2)).unwrap();
    let (status, body) = login_finish(&app, &login_id, &finished.finalization).await;
    assert_eq!(status, StatusCode::OK, "login/finish should authenticate");
    let v = parse(&body);
    assert_eq!(v["authenticated"], json!(true));
    assert_eq!(
        v["account_id"].as_str().unwrap(),
        reg_account_id,
        "login must resolve to the account_id from registration"
    );
}

/// Test 2: login with the wrong password fails. In OPAQUE the client itself
/// detects the failure at finish (it cannot recover its envelope / verify the
/// server), so it never even produces a KE3 — the strongest form of "fails".
#[tokio::test]
async fn login_with_wrong_password_fails() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"correct horse battery staple").await;

    let (flow, ke1) = ClientLoginFlow::start(b"WRONG password").unwrap();
    let (status, started) = login_start(&app, "alice", &ke1).await;
    assert_eq!(status, StatusCode::OK, "start still returns a (real) KE2");
    let (_login_id, ke2) = started.unwrap();

    // The client finalize must fail with a wrong password.
    let result = flow.finish(b"WRONG password", &b64d(&ke2));
    assert!(result.is_err(), "wrong password must fail client finalize");
}

/// Test 3a: at `login/start`, an unknown identity and a registered one return
/// the **same** response shape — 200 with a `login_id` and a
/// `credential_response_b64` of identical decoded length. No existence oracle at
/// the KE2 layer (§1.6).
#[tokio::test]
async fn login_start_shape_is_uniform_for_unknown_identity() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"correct horse battery staple").await;

    let (_f1, ke1_known) = ClientLoginFlow::start(b"pw").unwrap();
    let (s1, known) = login_start(&app, "alice", &ke1_known).await;
    let (_f2, ke1_unknown) = ClientLoginFlow::start(b"pw").unwrap();
    let (s2, unknown) = login_start(&app, "ghost-who-does-not-exist", &ke1_unknown).await;

    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    let (_id_k, ke2_known) = known.unwrap();
    let (_id_u, ke2_unknown) = unknown.unwrap();
    assert_eq!(
        b64d(&ke2_known).len(),
        b64d(&ke2_unknown).len(),
        "KE2 length must not reveal whether the account exists"
    );
}

/// Test 3b: the client-side finalize fails **identically** for a registered
/// identity with a wrong password and for an unknown identity. From the
/// client's vantage point the two are indistinguishable (dummy vs real-but-wrong
/// KE2 both fail to verify).
#[tokio::test]
async fn unknown_identity_client_failure_matches_wrong_password() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"correct horse battery staple").await;

    // Registered identity, wrong password.
    let (flow_a, ke1_a) = ClientLoginFlow::start(b"WRONG").unwrap();
    let (_s, started_a) = login_start(&app, "alice", &ke1_a).await;
    let (_id_a, ke2_a) = started_a.unwrap();
    let wrong_pw = flow_a.finish(b"WRONG", &b64d(&ke2_a));

    // Unknown identity (dummy path).
    let (flow_b, ke1_b) = ClientLoginFlow::start(b"WRONG").unwrap();
    let (_s, started_b) = login_start(&app, "ghost", &ke1_b).await;
    let (_id_b, ke2_b) = started_b.unwrap();
    let unknown = flow_b.finish(b"WRONG", &b64d(&ke2_b));

    assert!(wrong_pw.is_err(), "wrong password must fail");
    assert!(unknown.is_err(), "unknown identity must fail");
}

/// Test 3c: at the **server**'s `login/finish`, a wrong (mismatched) KE3 for a
/// registered identity and for an unknown identity yield byte-for-byte identical
/// `401` responses — the uniform failure. No existence oracle at the finish
/// layer either.
#[tokio::test]
async fn login_finish_401_is_byte_identical_for_known_and_unknown() {
    let (app, _auth) = multitenant_app();
    register(&app, "alice", b"correct horse battery staple").await;

    // A well-formed but transcript-mismatched KE3 (see `stale_finalization`).
    let stale = stale_finalization(&app, "alice", b"correct horse battery staple").await;

    // Submit it under a fresh login for the *registered* identity.
    let (_f, ke1_k) = ClientLoginFlow::start(b"pw").unwrap();
    let (_s, started_k) = login_start(&app, "alice", &ke1_k).await;
    let (login_id_k, _ke2_k) = started_k.unwrap();
    let (status_k, body_k) = login_finish(&app, &login_id_k, &stale).await;

    // ...and under a fresh login for an *unknown* identity (dummy path).
    let (_f, ke1_u) = ClientLoginFlow::start(b"pw").unwrap();
    let (_s, started_u) = login_start(&app, "ghost", &ke1_u).await;
    let (login_id_u, _ke2_u) = started_u.unwrap();
    let (status_u, body_u) = login_finish(&app, &login_id_u, &stale).await;

    assert_eq!(status_k, StatusCode::UNAUTHORIZED);
    assert_eq!(status_u, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_k, body_u,
        "the 401 body must be identical for known and unknown identities"
    );
}

/// Test 4 + 5: the stored `accounts` row holds the OPAQUE verifier **only** (no
/// password bytes), and the OPRF server secret (`ServerSetup`) is stored outside
/// the verifier rows (§1.5, §1.8).
#[tokio::test]
async fn stored_record_is_verifier_only_and_oprf_secret_is_separate() {
    let (app, auth) = multitenant_app();
    let password = b"super secret passphrase value";
    let handle = "alice";
    register(&app, handle, password).await;

    let record = {
        let store = auth.lock_store().unwrap();
        store.opaque_record(handle).unwrap().unwrap()
    };
    assert!(!record.is_empty());

    // The verifier must not contain the password (or the handle) verbatim.
    assert!(
        !contains_subseq(&record, password),
        "stored record must not contain the password"
    );
    assert!(
        !contains_subseq(&record, handle.as_bytes()),
        "stored record must not contain the identity handle"
    );

    // The OPRF server secret is a separate object held in AuthState, never
    // embedded in the per-account verifier row.
    let setup_bytes = auth.server_setup().serialize().to_vec();
    assert!(
        !contains_subseq(&record, &setup_bytes),
        "the OPRF server secret must not be stored inside the verifier row"
    );
}

/// Test 6: after enough failed logins for one identity, further attempts are
/// throttled with a uniform `429` (§1.7). Default threshold is 5, so the lockout
/// engages once the 6th failure is recorded.
#[tokio::test]
async fn repeated_failures_lock_out_one_identity() {
    let (app, _auth) = multitenant_app();
    let password = b"correct horse battery staple";
    register(&app, "alice", password).await;

    // A reusable well-formed-but-wrong KE3 that forces server-side 401s.
    let stale = stale_finalization(&app, "alice", password).await;

    let mut saw_lockout = false;
    for _ in 0..10 {
        let (flow, ke1) = ClientLoginFlow::start(password).unwrap();
        let (status, started) = login_start(&app, "alice", &ke1).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            saw_lockout = true;
            break;
        }
        assert_eq!(status, StatusCode::OK);
        let (login_id, _ke2) = started.unwrap();
        // Consume `flow` so the unused-variable lint is satisfied and the shapes
        // stay realistic; the mismatched `stale` KE3 is what forces the 401.
        drop(flow);
        let (fstatus, _) = login_finish(&app, &login_id, &stale).await;
        assert_eq!(fstatus, StatusCode::UNAUTHORIZED, "mismatched KE3 must 401");
    }
    assert!(
        saw_lockout,
        "one identity must eventually be locked out (429) after repeated failures"
    );
}

/// Test 7: blind mode does not mount the auth routes (`404`); multi-tenant mode
/// does (a well-formed request is handled, not `404`).
#[tokio::test]
async fn blind_mode_does_not_mount_auth_routes() {
    let blind = blind_app();
    let (status, _) = send_json(
        &blind,
        post(
            "/v1/auth/login/start",
            &json!({
                "identity_handle": "alice",
                "credential_request_b64": b64(b"whatever"),
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "blind mode must not expose the auth plane"
    );

    // Multi-tenant mode mounts them: a malformed credential request is a 400,
    // proving the route exists and is handled (not a 404).
    let (mt, _auth) = multitenant_app();
    let (status, _) = send_json(
        &mt,
        post(
            "/v1/auth/login/start",
            &json!({
                "identity_handle": "alice",
                "credential_request_b64": b64(b"not a real KE1"),
            }),
        ),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "multi-tenant mode must mount the auth plane"
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
