// SPDX-License-Identifier: AGPL-3.0-only

//! Real end-to-end-encryption integration tests for issue #125.
//!
//! Unlike `e2e.rs` (which uses a toy cipher to prove server blindness), these
//! drive the actual `pergamon-crypto` scheme through the HTTP router:
//!
//! - encrypt an event + convergent blob client-side, upload, pull, decrypt, and
//!   assert the plaintext round-trips while the server's on-disk bytes stay
//!   opaque; and
//! - exercise the opaque onboarding-artifact relay endpoints (device records,
//!   sealed enrollment/rotation bundles, attestations, recovery blob), proving
//!   the server relays them verbatim and the sealed secrets decrypt only
//!   client-side.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pergamon_crypto::device::DeviceKeypairs;
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_crypto::{
    EventHeader, RewrapRecipient, attest_trust, decrypt_blob, decrypt_event, enable_recovery,
    encrypt_blob, encrypt_event, open_enrollment_bundle, open_rewrapped, recover,
    rotate_and_rewrap, seal_enrollment_bundle,
};
use serde_json::{Value, json};
use tower::ServiceExt;

struct TempDb {
    path: std::path::PathBuf,
}

impl TempDb {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pergamon-sync-crypto-{}.db", uuid::Uuid::new_v4()));
        Self { path }
    }

    /// Every byte the server has persisted: the main database **plus** the WAL
    /// sidecar.
    ///
    /// Since WP-3e (#201) the store runs in WAL mode, so a just-committed row
    /// lives in `<db>-wal` until a checkpoint folds it into the main file.
    /// Reading only the main file would silently weaken the content-blindness
    /// assertions this suite exists for.
    fn all_bytes(&self) -> Vec<u8> {
        let mut bytes = std::fs::read(&self.path).unwrap();
        // DO NOT "simplify" this back to reading only the main file. Since
        // WP-3e (#201) the store runs in WAL mode, so a just-committed row lives
        // in `<db>-wal` until a checkpoint folds it into the main file. Reading
        // only the main file would make the "no plaintext" assertions pass
        // against bytes the server had not written there yet — i.e. it would
        // silently gut the content-blindness guarantee this suite exists for.
        // Both the negative assertions (no plaintext anywhere) and the positive
        // one (ciphertext present) must run against ALL persisted bytes.
        for suffix in ["-wal", "-shm"] {
            let sidecar = format!("{}{suffix}", self.path.display());
            if let Ok(mut extra) = std::fs::read(sidecar) {
                bytes.append(&mut extra);
            }
        }
        bytes
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

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

fn json_body(value: &Value) -> Body {
    Body::from(serde_json::to_vec(value).unwrap())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn hex32(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The headline #125 acceptance test: a real encrypt -> upload -> download ->
/// decrypt round-trip in which the server only ever holds ciphertext.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn real_crypto_event_and_blob_round_trip() {
    let tmp = TempDb::new();
    let store = pergamon_sync_server::SyncStore::open(&tmp.path).unwrap();
    let app = pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store));

    // --- Client key material -------------------------------------------------
    let ark = AccountRootKey::from_bytes([9u8; 32]);
    let account_id = AccountId::from_bytes([4u8; 16]);
    let account_hex = account_id.to_hex();
    let epoch = 0u32;
    let ack = ark.content_key(epoch).unwrap();

    // Distinctive plaintext markers that must never reach the server unencrypted.
    let blob_marker = format!("BLOB-PLAINTEXT-{}", uuid::Uuid::new_v4());
    let note_marker = format!("NOTE-PLAINTEXT-{}", uuid::Uuid::new_v4());

    // --- Encrypt the blob convergently and upload it -------------------------
    let blob = encrypt_blob(&ack, blob_marker.as_bytes()).unwrap();
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/{account_hex}/{}", blob.ct_hash))
            .body(Body::from(blob.ciphertext.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // --- Encrypt the event body (carrying the blob's plaintext_hash) ---------
    let change_id = uuid::Uuid::new_v4().to_string();
    let body_plain = json!({
        "note": note_marker,
        "blob_plaintext_hash": hex(&blob.plaintext_hash),
    });
    let header = EventHeader {
        protocol_version: 1,
        account_id: account_hex.clone(),
        device_id: "device-A".to_owned(),
        change_id: change_id.clone(),
        key_epoch: epoch,
        entity_ref: None,
        blob_refs: vec![blob.ct_hash.clone()],
    };
    let event_ct = encrypt_event(
        &ack,
        &header,
        serde_json::to_vec(&body_plain).unwrap().as_slice(),
    )
    .unwrap();

    let push = json!({
        "account_id": account_hex,
        "events": [{
            "protocol_version": 1,
            "account_id": account_hex,
            "device_id": "device-A",
            "change_id": change_id,
            "key_epoch": epoch,
            "blob_refs": [blob.ct_hash],
            "ciphertext_b64": STANDARD.encode(&event_ct),
        }]
    });
    let (status, _) = send(
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

    // --- The server database holds ciphertext only ---------------------------
    let db_bytes = tmp.all_bytes();
    assert!(
        !contains(&db_bytes, blob_marker.as_bytes()),
        "blob plaintext leaked into the server database"
    );
    assert!(
        !contains(&db_bytes, note_marker.as_bytes()),
        "event plaintext leaked into the server database"
    );
    assert!(
        contains(&db_bytes, &event_ct),
        "expected the ciphertext body to be stored verbatim"
    );

    // --- Pull + decrypt the event, then download + decrypt the blob ----------
    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v1/events?account_id={account_hex}&after=0"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pull: Value = serde_json::from_slice(&body).unwrap();
    let ev = &pull["events"][0];
    let pulled_ct = STANDARD
        .decode(ev["ciphertext_b64"].as_str().unwrap())
        .unwrap();

    // Rebuild the header exactly as the server echoes it (all AAD-bound fields).
    let pulled_header = EventHeader {
        protocol_version: u32::try_from(ev["protocol_version"].as_u64().unwrap()).unwrap(),
        account_id: ev["account_id"].as_str().unwrap().to_owned(),
        device_id: ev["device_id"].as_str().unwrap().to_owned(),
        change_id: ev["change_id"].as_str().unwrap().to_owned(),
        key_epoch: u32::try_from(ev["key_epoch"].as_u64().unwrap()).unwrap(),
        entity_ref: ev["entity_ref"].as_str().map(ToOwned::to_owned),
        blob_refs: ev["blob_refs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect(),
    };
    let recovered_body = decrypt_event(&ack, &pulled_header, &pulled_ct).unwrap();
    let recovered_json: Value = serde_json::from_slice(&recovered_body).unwrap();
    assert_eq!(recovered_json["note"], note_marker);

    let pt_hash = hex32(recovered_json["blob_plaintext_hash"].as_str().unwrap());
    let (status, blob_body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v1/blobs/{account_hex}/{}", blob.ct_hash))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let recovered_blob = decrypt_blob(&ack, &pt_hash, &blob_body).unwrap();
    assert_eq!(recovered_blob, blob_marker.as_bytes());
}

/// Convergent encryption keeps ADR-022 `ct_hash` dedup working under E2EE: the
/// same plaintext uploaded twice occupies one address.
#[tokio::test]
async fn convergent_blob_dedup_across_uploads() {
    let store = pergamon_sync_server::SyncStore::open_in_memory().unwrap();
    let app = pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store));
    let ark = AccountRootKey::from_bytes([1u8; 32]);
    let ack = ark.content_key(0).unwrap();
    let account = "acct-dedup";

    let a = encrypt_blob(&ack, b"identical bytes").unwrap();
    let b = encrypt_blob(&ack, b"identical bytes").unwrap();
    assert_eq!(a.ct_hash, b.ct_hash, "convergent encryption must be stable");

    for _ in 0..2 {
        let (status, _) = send(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/blobs/{account}/{}", a.ct_hash))
                .body(Body::from(a.ciphertext.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    // The probe reports the single address as present.
    let probe = json!({ "account_id": account, "ct_hashes": [a.ct_hash] });
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
    assert_eq!(resp["present"], json!([a.ct_hash]));
    assert!(resp["missing"].as_array().unwrap().is_empty());
}

/// The enrollment relay carries a sealed bundle the server cannot open; only the
/// target device recovers the ARK from it.
#[tokio::test]
async fn enrollment_bundle_relayed_and_opened_only_by_target() {
    let tmp = TempDb::new();
    let store = pergamon_sync_server::SyncStore::open(&tmp.path).unwrap();
    let app = pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store));

    let ark = AccountRootKey::from_bytes([7u8; 32]);
    let account_id = AccountId::from_bytes([2u8; 16]);
    let account_hex = account_id.to_hex();
    let new_device = DeviceKeypairs::generate().unwrap();

    // Existing device seals the bundle to the new device and relays it.
    let sealed = seal_enrollment_bundle(
        new_device.x25519_public(),
        new_device.device_id(),
        &ark,
        &account_id,
        0,
    )
    .unwrap();
    let (status, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!(
                "/v1/wraps/{account_hex}/{}",
                new_device.device_id()
            ))
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                &json!({ "bundle_b64": STANDARD.encode(&sealed) }),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The raw ARK bytes never appear in the server database.
    let db_bytes = tmp.all_bytes();
    assert!(!contains(&db_bytes, ark.expose_bytes()));

    // The new device lists and opens its bundle.
    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!(
                "/v1/wraps/{account_hex}/{}?after=0",
                new_device.device_id()
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed: Value = serde_json::from_slice(&body).unwrap();
    let relayed = STANDARD
        .decode(listed["bundles"][0]["bundle_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(relayed, sealed, "relay must return the bundle verbatim");

    let opened =
        open_enrollment_bundle(new_device.x25519_secret(), new_device.device_id(), &relayed)
            .unwrap();
    assert_eq!(opened.ark.expose_bytes(), ark.expose_bytes());
    assert_eq!(opened.account_id, account_id);
}

/// Device records and attestations relay verbatim and verify client-side.
#[tokio::test]
async fn device_records_and_attestations_relay() {
    let store = pergamon_sync_server::SyncStore::open_in_memory().unwrap();
    let app = pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store));

    let account = "acct-roster";
    let signer = DeviceKeypairs::generate().unwrap();
    let subject = DeviceKeypairs::generate().unwrap();
    let subject_record = subject.sign_record(1_700_000_000_000);

    // Opaque device-record bytes = signed body ‖ signature.
    let mut record_bytes = subject_record.record.signing_bytes();
    record_bytes.extend_from_slice(&subject_record.signature);
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/devices/{account}/{}", subject.device_id()))
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                &json!({ "record_b64": STANDARD.encode(&record_bytes) }),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // The signed device record verifies client-side.
    subject_record.verify().unwrap();

    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v1/devices/{account}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let roster: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(roster["devices"][0]["device_id"], subject.device_id());
    let relayed = STANDARD
        .decode(roster["devices"][0]["record_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(relayed, record_bytes);

    // A trust attestation relays and is content-deduplicated on re-submit.
    let att = attest_trust(&signer, &subject_record.record, 0, 1_700_000_000_100);
    att.verify().unwrap();
    let mut att_bytes = att.attestation.signing_bytes();
    att_bytes.extend_from_slice(&att.signature);
    let att_req = |bytes: &[u8]| {
        Request::builder()
            .method("POST")
            .uri(format!("/v1/attestations/{account}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                &json!({ "attestation_b64": STANDARD.encode(bytes) }),
            ))
            .unwrap()
    };
    let (status, body) = send(&app, att_req(&att_bytes)).await;
    assert_eq!(status, StatusCode::OK);
    let ack1: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(ack1["seq"], 1);
    assert_eq!(ack1["deduplicated"], false);

    let (status, body) = send(&app, att_req(&att_bytes)).await;
    assert_eq!(status, StatusCode::OK);
    let ack2: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(ack2["seq"], 1);
    assert_eq!(ack2["deduplicated"], true);
}

/// The recovery blob relays opaquely and only the passphrase recovers the ARK.
#[tokio::test]
async fn recovery_blob_relayed_and_recovered() {
    let tmp = TempDb::new();
    let store = pergamon_sync_server::SyncStore::open(&tmp.path).unwrap();
    let app = pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store));

    let ark = AccountRootKey::from_bytes([13u8; 32]);
    let account_id = AccountId::from_bytes([8u8; 16]);
    let account_hex = account_id.to_hex();
    let passphrase = b"correct horse battery staple";

    let blob = enable_recovery(&ark, &account_id, passphrase).unwrap();
    let (status, _) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v1/recovery/{account_hex}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                &json!({ "blob_b64": STANDARD.encode(blob.to_bytes()) }),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // The ARK never appears in the server database.
    let db_bytes = tmp.all_bytes();
    assert!(!contains(&db_bytes, ark.expose_bytes()));

    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v1/recovery/{account_hex}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: Value = serde_json::from_slice(&body).unwrap();
    let relayed = STANDARD.decode(resp["blob_b64"].as_str().unwrap()).unwrap();
    let parsed = pergamon_crypto::RecoveryBlob::from_bytes(&relayed).unwrap();

    let recovered = recover(&parsed, &account_id, passphrase).unwrap();
    assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
    assert!(recover(&parsed, &account_id, b"wrong").is_err());
}

/// A rotation re-wrap is relayed to the retained device and excludes the revoked
/// one.
#[tokio::test]
async fn rotation_rewrap_relayed_to_remaining_device() {
    let store = pergamon_sync_server::SyncStore::open_in_memory().unwrap();
    let app = pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store));

    let ark = AccountRootKey::from_bytes([5u8; 32]);
    let account_id = AccountId::from_bytes([6u8; 16]);
    let account_hex = account_id.to_hex();
    let keeper = DeviceKeypairs::generate().unwrap();

    let recipients = [RewrapRecipient {
        device_id: keeper.device_id(),
        x25519_pub: keeper.x25519_public(),
    }];
    let (ack1, wraps) = rotate_and_rewrap(&ark, &account_id, 1, &recipients).unwrap();

    let (status, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v1/wraps/{account_hex}/{}", keeper.device_id()))
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                &json!({ "bundle_b64": STANDARD.encode(&wraps[0].sealed) }),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!(
                "/v1/wraps/{account_hex}/{}?after=0",
                keeper.device_id()
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed: Value = serde_json::from_slice(&body).unwrap();
    let relayed = STANDARD
        .decode(listed["bundles"][0]["bundle_b64"].as_str().unwrap())
        .unwrap();

    let unwrapped = open_rewrapped(
        keeper.x25519_secret(),
        keeper.device_id(),
        &account_id,
        1,
        &relayed,
    )
    .unwrap();
    assert_eq!(unwrapped.expose_bytes(), ack1.expose_bytes());
}
