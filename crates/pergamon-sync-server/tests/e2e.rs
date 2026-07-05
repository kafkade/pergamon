// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end integration tests for the sync server.
//!
//! The headline test proves the acceptance criterion for issue #124: the
//! server stores and serves encrypted envelopes and **cannot read plaintext** —
//! the plaintext never appears in the server's on-disk database, and only a
//! client holding the key can recover it.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pergamon_sync_server::{AppState, SyncStore, build_router, ct_hash};
use serde_json::{Value, json};
use tower::ServiceExt;

/// A toy stream cipher standing in for a client-side AEAD. This test only needs
/// to prove the server stores opaque bytes it cannot read, so a trivial cipher
/// suffices here; the *real* `pergamon-crypto` end-to-end round-trip lives in
/// `e2e_crypto.rs` (#125).
fn xor_crypt(data: &[u8], key: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

/// A unique temp path for a test database, cleaned up by [`TempDb`].
struct TempDb {
    path: std::path::PathBuf,
}

impl TempDb {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pergamon-sync-test-{}.db", uuid::Uuid::new_v4()));
        Self { path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let p = format!("{}{suffix}", self.path.display());
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Send a request against a clone of the router and return status + body bytes.
async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

fn json_body(app_json: &Value) -> Body {
    Body::from(serde_json::to_vec(app_json).unwrap())
}

#[tokio::test]
async fn server_stores_ciphertext_only_and_round_trips() {
    let tmp = TempDb::new();
    let store = SyncStore::open(&tmp.path).unwrap();
    let app = build_router(AppState::new(store));

    let account = "acct-opaque-123";
    let device = "device-A";
    let key = b"super-secret-client-key";

    // Distinctive plaintext markers that must never touch the server unencrypted.
    let blob_plain = format!("BLOB-PLAINTEXT-MARKER-{}", uuid::Uuid::new_v4());
    let event_plain = format!("EVENT-PLAINTEXT-MARKER-{}", uuid::Uuid::new_v4());

    // Client encrypts locally.
    let blob_ct = xor_crypt(blob_plain.as_bytes(), key);
    let event_ct = xor_crypt(event_plain.as_bytes(), key);
    let blob_hash = ct_hash(&blob_ct);

    // 1. Upload the encrypted blob (content-addressed by ciphertext hash).
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/{account}/{blob_hash}"))
            .body(Body::from(blob_ct.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // 2. Push an event that references the blob, carrying an encrypted body.
    let change_id = uuid::Uuid::new_v4().to_string();
    let push = json!({
        "account_id": account,
        "events": [{
            "protocol_version": 1,
            "account_id": account,
            "device_id": device,
            "change_id": change_id,
            "key_epoch": 1,
            "blob_refs": [blob_hash],
            "ciphertext_b64": STANDARD.encode(&event_ct),
        }]
    });
    let (status, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/v1/events")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(&push))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let push_resp: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(push_resp["high_water_seq"], 1);
    assert_eq!(push_resp["results"][0]["server_seq"], 1);
    assert_eq!(push_resp["results"][0]["deduplicated"], false);

    // 3. The plaintext must not exist anywhere in the server's database file.
    let db_bytes = std::fs::read(&tmp.path).unwrap();
    assert!(
        !contains(&db_bytes, blob_plain.as_bytes()),
        "blob plaintext leaked into the server database"
    );
    assert!(
        !contains(&db_bytes, event_plain.as_bytes()),
        "event plaintext leaked into the server database"
    );
    // The ciphertext, by contrast, is present — the server holds opaque bytes.
    assert!(
        contains(&db_bytes, &event_ct),
        "expected the ciphertext to be stored verbatim"
    );

    // 4. Pull the event back and decrypt it client-side.
    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v1/events?account_id={account}&after=0"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pull: Value = serde_json::from_slice(&body).unwrap();
    let ev = &pull["events"][0];
    assert_eq!(ev["change_id"], change_id);
    assert_eq!(ev["server_seq"], 1);
    assert_eq!(ev["blob_refs"][0], blob_hash);
    let pulled_ct = STANDARD
        .decode(ev["ciphertext_b64"].as_str().unwrap())
        .unwrap();
    let recovered = xor_crypt(&pulled_ct, key);
    assert_eq!(recovered, event_plain.as_bytes());
    assert_eq!(pull["next_cursor"], 1);

    // 5. Download the blob and decrypt it client-side.
    let (status, blob_body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v1/blobs/{account}/{blob_hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(xor_crypt(&blob_body, key), blob_plain.as_bytes());
}

#[tokio::test]
async fn push_is_idempotent_on_change_id() {
    let store = SyncStore::open_in_memory().unwrap();
    let app = build_router(AppState::new(store));
    let account = "acct-1";
    let change_id = uuid::Uuid::new_v4().to_string();

    let push = json!({
        "account_id": account,
        "events": [{
            "protocol_version": 1,
            "account_id": account,
            "device_id": "d1",
            "change_id": change_id,
            "key_epoch": 1,
            "blob_refs": [],
            "ciphertext_b64": STANDARD.encode(b"opaque"),
        }]
    });

    let (s1, b1) = send(&app, post_events(&push)).await;
    assert_eq!(s1, StatusCode::OK);
    let r1: Value = serde_json::from_slice(&b1).unwrap();
    assert_eq!(r1["results"][0]["deduplicated"], false);
    assert_eq!(r1["results"][0]["server_seq"], 1);

    // Re-push the identical batch: dedupe, same server_seq, no new append.
    let (s2, b2) = send(&app, post_events(&push)).await;
    assert_eq!(s2, StatusCode::OK);
    let r2: Value = serde_json::from_slice(&b2).unwrap();
    assert_eq!(r2["results"][0]["deduplicated"], true);
    assert_eq!(r2["results"][0]["server_seq"], 1);
    assert_eq!(r2["high_water_seq"], 1);
}

#[tokio::test]
async fn event_referencing_missing_blob_is_rejected() {
    let store = SyncStore::open_in_memory().unwrap();
    let app = build_router(AppState::new(store));
    let account = "acct-2";

    let push = json!({
        "account_id": account,
        "events": [{
            "protocol_version": 1,
            "account_id": account,
            "device_id": "d1",
            "change_id": uuid::Uuid::new_v4().to_string(),
            "key_epoch": 1,
            "blob_refs": ["deadbeef-not-uploaded"],
            "ciphertext_b64": STANDARD.encode(b"opaque"),
        }]
    });

    let (status, _) = send(&app, post_events(&push)).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Nothing was appended.
    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v1/events?account_id={account}&after=0"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pull: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(pull["events"].as_array().unwrap().len(), 0);
    assert_eq!(pull["high_water_seq"], 0);
}

#[tokio::test]
async fn blob_put_rejects_wrong_hash() {
    let store = SyncStore::open_in_memory().unwrap();
    let app = build_router(AppState::new(store));

    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri("/v1/blobs/acct/not-the-real-hash")
            .body(Body::from(b"some ciphertext".to_vec()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn blob_probe_reports_present_and_missing() {
    let store = SyncStore::open_in_memory().unwrap();
    let app = build_router(AppState::new(store));
    let account = "acct-3";

    let ct = b"encrypted-blob-bytes".to_vec();
    let hash = ct_hash(&ct);

    // Upload one blob.
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/{account}/{hash}"))
            .body(Body::from(ct))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let probe = json!({ "account_id": account, "ct_hashes": [hash, "absent-hash"] });
    let (status, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/v1/blobs/probe")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(&probe))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp["present"], json!([hash]));
    assert_eq!(resp["missing"], json!(["absent-hash"]));
}

#[tokio::test]
async fn health_reports_ok() {
    let store = SyncStore::open_in_memory().unwrap();
    let app = build_router(AppState::new(store));
    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp["status"], "ok");
}

fn post_events(body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/events")
        .header(header::CONTENT_TYPE, "application/json")
        .body(json_body(body))
        .unwrap()
}

/// Naive substring search over raw bytes.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
