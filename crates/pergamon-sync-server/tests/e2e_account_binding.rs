// SPDX-License-Identifier: AGPL-3.0-only

//! Existing local identity through real TCP OPAQUE/PoP and the hardened relay.
//! Passing these tests is not external security certification.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use opaque_ke::ServerSetup;
use pergamon_core::sync::event::{BlobManifestEntry, ChangeBody, EntityType, Op};
use pergamon_crypto::{AccountId, AccountRootKey, DeviceKeypairs, RecoveryBlob};
use pergamon_storage::Database;
use pergamon_sync::{
    CryptoContext, DeviceKeyDirectory, HttpRelay, MemoryBlobStore, RelayTransport, SyncEngine,
    Transport, TransportCredential,
    account_binding::{self as binding, AuthTransport, BindingChallenge, RemoteSession},
    http::HttpTransport,
    http_auth::HttpAuth,
    wire::{BlobProbeRequest, PushRequest},
};
use pergamon_sync_server::{
    AbuseConfig, AppState, SyncStore,
    auth::{AuthState, PergamonCipherSuite, store::AuthStore, throttle::ThrottleConfig},
    build_router_multitenant_hardened,
};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use std::path::PathBuf;

const PASSWORD: &[u8] = b"binding-fixture-password-not-content-recovery";

fn v2_registration_response(
    url: &str,
    identity: &str,
    password: &[u8],
) -> (reqwest::StatusCode, Vec<u8>) {
    let auth = HttpAuth::new(url).unwrap();
    let (flow, message) = pergamon_sync::auth::ClientRegistrationFlow::start(password).unwrap();
    let start = auth
        .request(
            "/v2/auth/register/start",
            Some(&json!({
                "identity_handle":identity,"registration_request_b64":STANDARD.encode(message),
            })),
            None,
        )
        .unwrap();
    let upload = flow
        .finish(
            password,
            &STANDARD
                .decode(start["registration_response_b64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
    let response = reqwest::blocking::Client::new()
        .post(format!("{url}/v2/auth/register/finish"))
        .json(
            &json!({"identity_handle":identity,"registration_upload_b64":STANDARD.encode(upload)}),
        )
        .send()
        .unwrap();
    (response.status(), response.bytes().unwrap().to_vec())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_registration_is_uniform_and_never_replaces_an_existing_credential() {
    let server = Server::new().await;
    let url = server.url.clone();
    let state = server.auth.clone();
    tokio::task::spawn_blocking(move || {
        let expected=br#"{"registration_received":true,"requires_login":true}"#;
        let first=v2_registration_response(&url,"uniform-owner",PASSWORD);
        assert_eq!(first.0,reqwest::StatusCode::OK);
        assert_eq!(first.1,expected);
        let original=state.lock_store().unwrap().opaque_record("uniform-owner").unwrap().unwrap();
        let tenant=state.lock_store().unwrap().account_id("uniform-owner").unwrap().unwrap();
        for (identity,password) in [
            ("uniform-owner",b"wrong-new-credential".as_slice()),
            ("uniform-owner",PASSWORD),("another-new-owner",PASSWORD),
        ] {
            let response=v2_registration_response(&url,identity,password);
            assert_eq!(response,first,"new/existing handles have identical status and literal body");
        }
        assert_eq!(state.lock_store().unwrap().opaque_record("uniform-owner").unwrap().unwrap(),original);
        assert_eq!(state.lock_store().unwrap().account_id("uniform-owner").unwrap().unwrap(),tenant);
        let auth=HttpAuth::new(&url).unwrap();let keys=DeviceKeypairs::generate().unwrap();
        assert!(binding::login_device(&auth,"uniform-owner",b"wrong-new-credential",&keys).is_err());
        let login=binding::login_device(&auth,"uniform-owner",PASSWORD,&keys).unwrap();
        assert_eq!(login.auth_tenant_id,tenant);
        assert!(login.content_account_id.is_none());
        let (_,message)=pergamon_sync::auth::ClientLoginFlow::start(b"wrong-new-credential").unwrap();
        let start=auth.request("/v2/auth/login/start",Some(&json!({
            "identity_handle":"uniform-owner","credential_request_b64":STANDARD.encode(message),
        })),None).unwrap();
        let response=reqwest::blocking::Client::new().post(format!("{url}/v2/auth/login/finish"))
            .json(&json!({"login_id":start["login_id"],"credential_finalization_b64":STANDARD.encode([0_u8;64])}))
            .send().unwrap();
        assert_eq!(response.status(),reqwest::StatusCode::UNAUTHORIZED);
        let failure:Value=response.json().unwrap();
        assert!(failure.get("auth_tenant_id").is_none());
        assert!(failure.get("content_account_id").is_none());
        assert!(failure.get("token").is_none());
    }).await.unwrap();
}

struct Server {
    url: String,
    state: AppState,
    auth: AuthState,
    task: tokio::task::JoinHandle<()>,
    content_path: PathBuf,
    auth_path: PathBuf,
}

impl Server {
    async fn new() -> Self {
        let prefix =
            std::env::temp_dir().join(format!("pergamon-binding-{}", uuid::Uuid::new_v4()));
        Self::open(
            prefix.with_extension("content.db"),
            prefix.with_extension("auth.db"),
            None,
        )
        .await
    }

    async fn open(content_path: PathBuf, auth_path: PathBuf, setup: Option<Vec<u8>>) -> Self {
        let state = AppState::new(SyncStore::open(&content_path).unwrap());
        let setup = setup.map_or_else(
            || ServerSetup::<PergamonCipherSuite>::new(&mut OsRng),
            |bytes| ServerSetup::deserialize(&bytes).unwrap(),
        );
        let auth = AuthState::new(
            AuthStore::open(&auth_path).unwrap(),
            setup,
            "fixture-v1",
            ThrottleConfig::default(),
        );
        let abuse = AbuseConfig {
            strict_rate_limit_burst: 2000,
            ..AbuseConfig::default()
        };
        let app = build_router_multitenant_hardened(state.clone(), auth.clone(), &abuse);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            url,
            state,
            auth,
            task,
            content_path,
            auth_path,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for path in [&self.content_path, &self.auth_path] {
            for suffix in ["", "-wal", "-shm"] {
                let file = PathBuf::from(format!("{}{suffix}", path.display()));
                match std::fs::read(file) {
                    Ok(bytes) => out.extend(bytes),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("reading server persistence: {error}"),
                }
            }
        }
        out
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        for path in [&self.content_path, &self.auth_path] {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
        }
    }
}

fn session(auth: &HttpAuth, identity: &str, keys: &DeviceKeypairs) -> RemoteSession {
    binding::register_identity(auth, identity, PASSWORD).unwrap();
    binding::login_device(auth, identity, PASSWORD, keys).unwrap()
}

fn transport(url: &str, session: &RemoteSession) -> HttpTransport {
    HttpTransport::with_credential(
        url,
        Some(TransportCredential::Bearer {
            token: session.access_token.clone(),
        }),
    )
    .unwrap()
}

fn relay(url: &str, session: &RemoteSession) -> HttpRelay {
    HttpRelay::with_credential(
        url,
        Some(TransportCredential::Bearer {
            token: session.access_token.clone(),
        }),
    )
    .unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for byte in bytes {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}

fn assert_refusal(error: &pergamon_sync::SyncError, status: u16, code: &str) {
    assert!(
        matches!(error,pergamon_sync::SyncError::AuthRefused {status:s,code:c}
        if *s==status && c==code),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_existing_local_artifacts_survive_authenticated_attach() {
    let server = Server::new().await;
    let url = server.url.clone();
    let result = tokio::task::spawn_blocking(move || {
        // Everything below is created before the first PAKE message.
        let id = AccountId::generate().unwrap();
        let content = id.to_hex();
        let ark = AccountRootKey::from_bytes([73; 32]);
        let keys = DeviceKeypairs::generate().unwrap();
        let peer = DeviceKeypairs::generate().unwrap();
        let original_keys = (*keys.x25519_secret(), *keys.ed25519_signing());
        let crypto = CryptoContext::new(
            ark.clone(),
            content.clone(),
            keys.device_id().to_owned(),
            *keys.ed25519_signing(),
            0,
        )
        .unwrap();
        let plaintext = format!("LOCAL-CONTENT-BEFORE-AUTH-{}", uuid::Uuid::new_v4());
        let blob_plaintext = format!("LOCAL-BLOB-BEFORE-AUTH-{}", uuid::Uuid::new_v4());
        let blob = crypto
            .encrypt_blob_plaintext(blob_plaintext.as_bytes())
            .unwrap();
        let db = Database::open_in_memory().unwrap();
        db.set_sync_identity(&content, keys.device_id(), 0, None)
            .unwrap();
        let document = uuid::Uuid::new_v4().to_string();
        let fields = json!({"url":"https://binding.example/existing","title":plaintext,
            "content_text":plaintext,"content_type":"article","status":"inbox"})
        .as_object()
        .unwrap()
        .clone();
        db.emit_change(
            EntityType::Document,
            &document,
            Op::Upsert,
            fields,
            vec![BlobManifestEntry {
                ct_hash: blob.ct_hash.clone(),
                role: "raw_html".to_owned(),
                plaintext_hash: hex(&blob.plaintext_hash),
                plaintext_len: blob_plaintext.len().try_into().unwrap(),
            }],
            1_700_000_000_000,
        )
        .unwrap();
        let outbox = db.pending_outbox(1).unwrap().remove(0);
        let body = ChangeBody::from_bytes(&outbox.body).unwrap();
        let event = crypto.encrypt_change(&outbox.change_id, &body).unwrap();
        let record = keys.sign_record(1_700_000_000_000).to_bytes();
        let attestation = pergamon_crypto::attest_trust(
            &keys,
            &keys.sign_record(1_700_000_000_000).record,
            0,
            1_700_000_000_000,
        )
        .to_bytes();
        let sealed = pergamon_crypto::seal_enrollment_bundle(
            peer.x25519_public(),
            peer.device_id(),
            &ark,
            &id,
            0,
        )
        .unwrap();
        let recovery = pergamon_crypto::enable_recovery(&ark, &id, b"existing-recovery-secret")
            .unwrap()
            .to_bytes();
        let package =
            pergamon_crypto::export_key_package(&ark, &id, b"existing-package-secret").unwrap();
        let (_, rewraps) = pergamon_crypto::rotate_and_rewrap(
            &ark,
            &id,
            1,
            &[pergamon_crypto::RewrapRecipient {
                device_id: peer.device_id(),
                x25519_pub: peer.x25519_public(),
            }],
        )
        .unwrap();

        let auth = HttpAuth::new(&url).unwrap();
        let control = session(&auth, "local-owner", &keys);
        assert_ne!(control.auth_tenant_id, content);
        assert!(control.content_account_id.is_none());
        assert!(transport(&url, &control).pull(&content, 0, None).is_err());
        let receipt = binding::bind_empty_namespace(
            &auth,
            &control,
            &keys,
            &content,
            "existing-local-attach",
        )
        .unwrap();
        assert_eq!(receipt.content_account_id, content);
        assert!(
            binding::binding_status(&auth, &control).is_err(),
            "old control token must be revoked"
        );
        let active = binding::login_device(&auth, "local-owner", PASSWORD, &keys).unwrap();
        assert_eq!(active.content_account_id.as_deref(), Some(content.as_str()));
        assert_eq!(
            binding::bind_empty_namespace(&auth, &active, &keys, &content, "existing-local-attach")
                .unwrap(),
            receipt
        );
        let remote = transport(&url, &active);
        let artifacts = relay(&url, &active);
        artifacts
            .device_put(&content, keys.device_id(), &record)
            .unwrap();
        artifacts
            .attestation_append(&content, &attestation)
            .unwrap();
        artifacts
            .wrap_put(&content, peer.device_id(), &sealed)
            .unwrap();
        artifacts.recovery_put(&content, &recovery).unwrap();
        remote
            .blob_put(&content, &blob.ct_hash, &blob.ciphertext)
            .unwrap();
        remote
            .push(&PushRequest {
                account_id: content.clone(),
                events: vec![event.clone()],
            })
            .unwrap();
        let echoed = remote.pull(&content, 0, None).unwrap().events.remove(0);
        assert_eq!(echoed.ciphertext_b64, event.ciphertext_b64);
        assert_eq!(echoed.sig_b64, event.sig_b64);
        assert_eq!(echoed.account_id, content);
        assert_eq!(
            artifacts
                .device_get(&content, keys.device_id())
                .unwrap()
                .unwrap(),
            record
        );
        assert_eq!(
            artifacts.wraps_list(&content, peer.device_id(), 0).unwrap()[0].bundle,
            sealed
        );
        assert_eq!(artifacts.recovery_get(&content).unwrap().unwrap(), recovery);
        assert_eq!(
            artifacts.attestations_list(&content, 0).unwrap()[0].attestation,
            attestation
        );
        assert_eq!(
            pergamon_crypto::recover(
                &RecoveryBlob::from_bytes(&recovery).unwrap(),
                &id,
                b"existing-recovery-secret"
            )
            .unwrap()
            .expose_bytes(),
            ark.expose_bytes()
        );
        assert_eq!(
            pergamon_crypto::import_key_package(&package, b"existing-package-secret")
                .unwrap()
                .expose_bytes(),
            ark.expose_bytes()
        );
        assert!(
            pergamon_crypto::recover(&RecoveryBlob::from_bytes(&recovery).unwrap(), &id, b"wrong")
                .is_err()
        );
        let accepted = pergamon_crypto::open_enrollment_bundle(
            peer.x25519_secret(),
            peer.device_id(),
            &sealed,
        )
        .unwrap();
        assert_eq!(accepted.account_id, id);
        assert_eq!(accepted.ark.expose_bytes(), ark.expose_bytes());
        let rotated = pergamon_crypto::open_rewrapped(
            peer.x25519_secret(),
            peer.device_id(),
            &id,
            1,
            &rewraps[0].sealed,
        )
        .unwrap();
        assert_eq!(
            rotated.expose_bytes(),
            ark.content_key(1).unwrap().expose_bytes()
        );
        let peer_session = binding::login_device(&auth, "local-owner", PASSWORD, &peer).unwrap();
        pergamon_sync::onboarding::enroll_publish(
            &relay(&url, &peer_session),
            &id,
            &peer,
            1_700_000_000_001,
        )
        .unwrap();
        let mut directory = DeviceKeyDirectory::new();
        directory.insert(keys.device_id(), *keys.ed25519_verifying());
        let peer_crypto = CryptoContext::new(
            accepted.ark,
            content.clone(),
            peer.device_id().to_owned(),
            *peer.ed25519_signing(),
            0,
        )
        .unwrap();
        let peer_db = Database::open_in_memory().unwrap();
        peer_db
            .set_sync_identity(&content, peer.device_id(), 0, None)
            .unwrap();
        let blobs = MemoryBlobStore::new();
        let engine = SyncEngine::new(transport(&url, &peer_session), peer_crypto, directory);
        assert_eq!(engine.pull(&peer_db, &blobs).unwrap(), 1);
        assert_eq!(
            peer_db
                .read_entity_fields(EntityType::Document, &document)
                .unwrap()
                .unwrap()["title"],
            plaintext
        );
        assert_eq!(
            pergamon_sync::BlobStore::load(&blobs, &hex(&blob.plaintext_hash))
                .unwrap()
                .unwrap(),
            blob_plaintext.as_bytes()
        );
        assert_eq!(
            db.sync_state().unwrap().account_id.as_deref(),
            Some(content.as_str())
        );
        assert_eq!(
            (*keys.x25519_secret(), *keys.ed25519_signing()),
            original_keys
        );
        let mut wrong = echoed;
        wrong.account_id = active.auth_tenant_id.clone();
        assert!(crypto.decrypt_change(&wrong).is_err());
        assert!(
            !crypto
                .verify_event_sig(&wrong, keys.ed25519_verifying())
                .unwrap()
        );
        assert!(
            remote.pull(&active.auth_tenant_id, 0, None).is_err(),
            "tenant ID is not an alias"
        );
        let other_keys = DeviceKeypairs::generate().unwrap();
        let other = session(&auth, "another-owner", &other_keys);
        let claim =
            binding::bind_empty_namespace(&auth, &other, &other_keys, &content, "steal-existing")
                .unwrap_err();
        assert_refusal(&claim, 409, "CONTENT_NAMESPACE_UNAVAILABLE");
        let refreshed = binding::refresh_session(&auth, &keys, &active).unwrap();
        assert!(
            binding::refresh_session(&auth, &keys, &active).is_err(),
            "refresh is single-use"
        );
        assert!(binding::refresh_session(&auth, &other_keys, &refreshed).is_err());
        binding::revoke_device_session(&auth, &refreshed, keys.device_id()).unwrap();
        assert!(transport(&url, &refreshed).pull(&content, 0, None).is_err());
        (
            plaintext,
            blob_plaintext,
            *ark.expose_bytes(),
            STANDARD.decode(event.ciphertext_b64).unwrap(),
        )
    })
    .await
    .unwrap();
    let persisted = server.bytes();
    for forbidden in [
        result.0.as_bytes(),
        result.1.as_bytes(),
        result.2.as_slice(),
        PASSWORD,
    ] {
        assert!(!persisted.windows(forbidden.len()).any(|w| w == forbidden));
    }
    assert!(
        persisted.windows(result.3.len()).any(|w| w == result.3),
        "positive ciphertext control"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_occupied_blind_table_refuses_first_claim() {
    let server = Server::new().await;
    let state = server.state.clone();
    let url = server.url.clone();
    tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        for (index, kind) in ["blob", "device", "wrap", "attestation", "recovery", "event"]
            .iter()
            .enumerate()
        {
            let id = AccountId::generate().unwrap().to_hex();
            match *kind {
                "blob" => state
                    .store
                    .blob_put(
                        &id,
                        &pergamon_sync_server::ct_hash(b"ciphertext"),
                        b"ciphertext",
                    )
                    .unwrap(),
                "device" => state
                    .store
                    .device_record_put(&id, "old", b"unanchored-self-signature")
                    .unwrap(),
                "wrap" => {
                    state
                        .store
                        .wrapped_bundle_put(&id, "old", b"old-wrap")
                        .unwrap();
                }
                "attestation" => {
                    state
                        .store
                        .attestation_append(&id, b"old-attestation")
                        .unwrap();
                }
                "recovery" => state.store.recovery_blob_put(&id, b"old-recovery").unwrap(),
                "event" => {
                    state
                        .store
                        .push_events(
                            &id,
                            &[pergamon_sync_server::store::EventRecord {
                                protocol_version: 1,
                                account_id: id.clone(),
                                device_id: "old".to_owned(),
                                change_id: "old".to_owned(),
                                entity_ref: None,
                                key_epoch: 0,
                                blob_refs: vec![],
                                ciphertext: b"old-event".to_vec(),
                                signature: vec![],
                            }],
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let keys = DeviceKeypairs::generate().unwrap();
            let control = session(&auth, &format!("blind-claim-{index}"), &keys);
            let error = binding::bind_empty_namespace(&auth, &control, &keys, &id, "refuse-legacy")
                .unwrap_err();
            assert_refusal(&error, 409, "CONTENT_NAMESPACE_UNAVAILABLE");
            assert!(
                binding::binding_status(&auth, &control)
                    .unwrap()
                    .binding
                    .is_none()
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn challenge_proof_is_tenant_device_target_and_operation_bound() {
    let server = Server::new().await;
    let url = server.url.clone();
    tokio::task::spawn_blocking(move || {
        let auth=HttpAuth::new(&url).unwrap();
        let keys=DeviceKeypairs::generate().unwrap();
        let attacker=DeviceKeypairs::generate().unwrap();
        let control=session(&auth,"proof-owner",&keys);
        let other=session(&auth,"proof-attacker",&attacker);
        let id=AccountId::generate().unwrap().to_hex();
        let request=json!({"operation_id":"proof-op","content_account_id":id});
        let challenge:BindingChallenge=serde_json::from_value(auth.request(
            "/v2/auth/content-binding/start",Some(&request),Some(&control.access_token)).unwrap()).unwrap();
        let server_challenge:pergamon_sync_server::auth::binding::BindingChallenge=
            serde_json::from_value(serde_json::to_value(&challenge).unwrap()).unwrap();
        let bytes=binding::binding_signing_bytes(&challenge).unwrap();
        assert_eq!(bytes,pergamon_sync_server::auth::binding::signing_bytes(&server_challenge).unwrap());
        let altered=json!({"operation_id":"proof-op","content_account_id":AccountId::generate().unwrap().to_hex()});
        assert!(auth.request("/v2/auth/content-binding/start",Some(&altered),Some(&control.access_token)).is_err());
        let mut finish=json!({"operation_id":"proof-op","challenge_id":challenge.challenge_id,
            "pop_signature_b64":STANDARD.encode(attacker.sign(&bytes))});
        assert!(auth.request("/v2/auth/content-binding/finish",Some(&finish),Some(&control.access_token)).is_err());
        finish["pop_signature_b64"]=json!(STANDARD.encode(keys.sign(&bytes)));
        assert!(auth.request("/v2/auth/content-binding/finish",Some(&finish),Some(&other.access_token)).is_err());
        assert!(auth.request("/v2/auth/content-binding/finish",Some(&finish),None).is_err());
        auth.request("/v2/auth/content-binding/finish",Some(&finish),Some(&control.access_token)).unwrap();
        let active=binding::login_device(&auth,"proof-owner",PASSWORD,&keys).unwrap();
        assert_eq!(auth.request("/v2/auth/content-binding/finish",Some(&finish),Some(&active.access_token)).unwrap()["content_account_id"],id);
        let status=binding::binding_status(&auth,&active).unwrap();
        assert_eq!(status.binding.unwrap().content_account_id,id);
        assert!(binding::bind_empty_namespace(&auth,&active,&keys,&AccountId::generate().unwrap().to_hex(),"rebind").is_err());
        let naked=reqwest::blocking::Client::new();
        assert_eq!(naked.get(format!("{url}/v1/usage/{id}")).send().unwrap().status(),401);
        assert_eq!(naked.post(format!("{url}/v1/blobs/probe")).bearer_auth(&other.access_token)
            .json(&json!({"account_id":id,"ct_hashes":[]})).send().unwrap().status(),403);
        assert_eq!(naked.get(format!("{url}/v1/usage/{id}")).bearer_auth(&active.access_token).send().unwrap().status(),200);
        assert!(transport(&url,&active).blob_probe(&BlobProbeRequest {account_id:other.auth_tenant_id,ct_hashes:vec![]}).is_err());
    }).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_binding_reply_and_restart_resume_the_same_receipt() {
    let server = Server::new().await;
    let url = server.url.clone();
    let setup = server.auth.server_setup().serialize().to_vec();
    let material = tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        let keys = DeviceKeypairs::generate().unwrap();
        let control = session(&auth, "restart-owner", &keys);
        let id = AccountId::generate().unwrap().to_hex();
        let receipt =
            binding::bind_empty_namespace(&auth, &control, &keys, &id, "lost-reply").unwrap();
        (keys, id, receipt)
    })
    .await
    .unwrap();
    let content_path = server.content_path.clone();
    let auth_path = server.auth_path.clone();
    server.task.abort();
    let state = server.state.clone();
    let auth = server.auth.clone();
    // Keep the paths owned by a replacement server rather than deleting them on drop.
    let mut old = server;
    old.content_path = content_path.with_extension("unused");
    old.auth_path = auth_path.with_extension("unused");
    drop(old);
    drop(state);
    drop(auth);
    let restarted = Server::open(content_path, auth_path, Some(setup)).await;
    let url = restarted.url.clone();
    tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        let active = binding::login_device(&auth, "restart-owner", PASSWORD, &material.0).unwrap();
        assert_eq!(
            binding::bind_empty_namespace(&auth, &active, &material.0, &material.1, "lost-reply")
                .unwrap(),
            material.2
        );
        assert_eq!(
            binding::binding_status(&auth, &active)
                .unwrap()
                .binding
                .unwrap(),
            material.2
        );
        binding::register_identity(&auth, "restart-owner", b"different-registration-password")
            .unwrap();
        assert!(binding::login_device(&auth, "restart-owner", PASSWORD, &material.0).is_ok());
        assert!(
            binding::login_device(
                &auth,
                "restart-owner",
                b"different-registration-password",
                &material.0
            )
            .is_err()
        );
    })
    .await
    .unwrap();
}

fn legacy_registration(auth: &HttpAuth, identity: &str) -> String {
    use pergamon_sync::auth::ClientRegistrationFlow;
    let (flow, message) = ClientRegistrationFlow::start(PASSWORD).unwrap();
    let response = auth
        .request(
            "/v1/auth/register/start",
            Some(&json!({
                "identity_handle":identity,"registration_request_b64":STANDARD.encode(message),
            })),
            None,
        )
        .unwrap();
    let upload = flow
        .finish(
            PASSWORD,
            &STANDARD
                .decode(response["registration_response_b64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
    auth.request(
        "/v1/auth/register/finish",
        Some(&json!({
            "identity_handle":identity,"registration_upload_b64":STANDARD.encode(upload),
        })),
        None,
    )
    .unwrap()["account_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn legacy_login(
    auth: &HttpAuth,
    identity: &str,
    keys: &DeviceKeypairs,
) -> pergamon_sync::error::Result<Value> {
    use pergamon_sync::auth::{ClientLoginFlow, build_mint_pop};
    let (flow, message) = ClientLoginFlow::start(PASSWORD).unwrap();
    let response = auth.request(
        "/v1/auth/login/start",
        Some(&json!({
            "identity_handle":identity,"credential_request_b64":STANDARD.encode(message),
        })),
        None,
    )?;
    let login = response["login_id"].as_str().unwrap();
    let finished = flow
        .finish(
            PASSWORD,
            &STANDARD
                .decode(response["credential_response_b64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
    let proof = build_mint_pop(keys, login, &finished.finalization);
    auth.request(
        "/v1/auth/login/finish",
        Some(&json!({
            "login_id":login,"credential_finalization_b64":STANDARD.encode(finished.finalization),
            "device_id":proof.device_id,"ed25519_pub_b64":proof.ed25519_pub_b64,
            "pop_signature_b64":proof.pop_signature_b64,
        })),
        None,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_empty_transition_revokes_tokens_but_occupied_identity_is_immutable() {
    let server = Server::new().await;
    let url = server.url.clone();
    tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        let keys = DeviceKeypairs::generate().unwrap();
        let tenant = legacy_registration(&auth, "legacy-unused");
        let old = legacy_login(&auth, "legacy-unused", &keys).unwrap();
        assert_eq!(old["account_id"], tenant);
        assert_eq!(old["token"]["account_id"], tenant);
        let legacy = binding::login_device(&auth, "legacy-unused", PASSWORD, &keys).unwrap();
        let content = AccountId::generate().unwrap().to_hex();
        binding::bind_empty_namespace(&auth, &legacy, &keys, &content, "compatibility").unwrap();
        let client = reqwest::blocking::Client::new();
        assert_eq!(
            client
                .get(format!("{url}/v1/usage/{tenant}"))
                .bearer_auth(old["token"]["access_token"].as_str().unwrap())
                .send()
                .unwrap()
                .status(),
            401
        );
        assert!(binding::refresh_session(&auth, &keys, &legacy).is_err());
        assert_refusal(
            &legacy_login(&auth, "legacy-unused", &keys).unwrap_err(),
            409,
            "AUTH_PROTOCOL_UPGRADE_REQUIRED",
        );
        let active = binding::login_device(&auth, "legacy-unused", PASSWORD, &keys).unwrap();
        assert_eq!(active.auth_tenant_id, tenant);
        assert_eq!(active.content_account_id.as_deref(), Some(content.as_str()));
        assert!(transport(&url, &active).pull(&tenant, 0, None).is_err());
        let attacker = DeviceKeypairs::generate().unwrap();
        let other = session(&auth, "claim-retired", &attacker);
        assert_refusal(
            &binding::bind_empty_namespace(&auth, &other, &attacker, &tenant, "retired")
                .unwrap_err(),
            409,
            "CONTENT_NAMESPACE_UNAVAILABLE",
        );
        let occupied = legacy_registration(&auth, "legacy-occupied");
        let occupied_session =
            binding::login_device(&auth, "legacy-occupied", PASSWORD, &keys).unwrap();
        transport(&url, &occupied_session)
            .blob_put(
                &occupied,
                &pergamon_sync_server::ct_hash(b"legacy-ciphertext"),
                b"legacy-ciphertext",
            )
            .unwrap();
        assert_refusal(
            &binding::bind_empty_namespace(
                &auth,
                &occupied_session,
                &keys,
                &AccountId::generate().unwrap().to_hex(),
                "no-reid",
            )
            .unwrap_err(),
            409,
            "BINDING_IMMUTABLE",
        );
        assert_eq!(
            legacy_login(&auth, "legacy-occupied", &keys).unwrap()["account_id"],
            occupied
        );
        assert_eq!(
            binding::binding_status(&auth, &occupied_session)
                .unwrap()
                .binding
                .unwrap()
                .content_account_id,
            occupied
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_identity_is_enforced_on_every_content_route() {
    let server = Server::new().await;
    let url = server.url.clone();
    let auth_state = server.auth.clone();
    tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        let a_keys = DeviceKeypairs::generate().unwrap();
        let b_keys = DeviceKeypairs::generate().unwrap();
        let a_control = session(&auth, "routes-a", &a_keys);
        let b_control = session(&auth, "routes-b", &b_keys);
        let a_id = AccountId::generate().unwrap().to_hex();
        let b_id = AccountId::generate().unwrap().to_hex();
        binding::bind_empty_namespace(&auth, &a_control, &a_keys, &a_id, "a").unwrap();
        binding::bind_empty_namespace(&auth, &b_control, &b_keys, &b_id, "b").unwrap();
        let a = binding::login_device(&auth, "routes-a", PASSWORD, &a_keys).unwrap();
        let raw = reqwest::blocking::Client::new();
        let routes = [
            ("GET", format!("/v1/events?account_id={b_id}&after=0"), None),
            (
                "POST",
                "/v1/events".to_owned(),
                Some(json!({"account_id":b_id,"events":[]})),
            ),
            (
                "POST",
                "/v1/blobs/probe".to_owned(),
                Some(json!({"account_id":b_id,"ct_hashes":[]})),
            ),
            ("GET", format!("/v1/blobs/{b_id}/hash"), None),
            ("PUT", format!("/v1/blobs/{b_id}/hash"), Some(json!({}))),
            ("GET", format!("/v1/devices/{b_id}"), None),
            ("GET", format!("/v1/devices/{b_id}/device"), None),
            (
                "PUT",
                format!("/v1/devices/{b_id}/device"),
                Some(json!({"record_b64":""})),
            ),
            ("GET", format!("/v1/wraps/{b_id}/device"), None),
            (
                "POST",
                format!("/v1/wraps/{b_id}/device"),
                Some(json!({"bundle_b64":""})),
            ),
            ("GET", format!("/v1/attestations/{b_id}"), None),
            (
                "POST",
                format!("/v1/attestations/{b_id}"),
                Some(json!({"attestation_b64":""})),
            ),
            ("GET", format!("/v1/recovery/{b_id}"), None),
            (
                "PUT",
                format!("/v1/recovery/{b_id}"),
                Some(json!({"blob_b64":""})),
            ),
            ("GET", format!("/v1/usage/{b_id}"), None),
        ];
        for (method, path, body) in &routes {
            let request = raw.request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                format!("{url}{path}"),
            );
            let request = body.as_ref().map_or_else(
                || request.try_clone().unwrap(),
                |body| request.try_clone().unwrap().json(body),
            );
            assert_eq!(
                request
                    .try_clone()
                    .unwrap()
                    .bearer_auth(&a.access_token)
                    .send()
                    .unwrap()
                    .status(),
                403,
                "{method} {path}"
            );
            assert_eq!(request.send().unwrap().status(), 401, "{method} {path}");
        }
        auth_state
            .lock_store()
            .unwrap()
            .revoke_device(&a.auth_tenant_id, a_keys.device_id())
            .unwrap();
        assert_eq!(
            raw.get(format!("{url}/v1/usage/{a_id}"))
                .bearer_auth(&a.access_token)
                .send()
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            raw.get(format!("{url}/health")).send().unwrap().status(),
            200
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_first_claim_has_exactly_one_owner() {
    let server = Server::new().await;
    let url = server.url.clone();
    tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        let a_keys = DeviceKeypairs::generate().unwrap();
        let b_keys = DeviceKeypairs::generate().unwrap();
        let a = session(&auth, "race-a", &a_keys);
        let b = session(&auth, "race-b", &b_keys);
        let id = AccountId::generate().unwrap().to_hex();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        // Both contenders must be spawned before any join waits on the barrier.
        #[allow(clippy::needless_collect)]
        let handles = [(a, a_keys), (b, b_keys)]
            .into_iter()
            .enumerate()
            .map(|(i, (session, keys))| {
                let url = url.clone();
                let id = id.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let auth = HttpAuth::new(&url).unwrap();
                    barrier.wait();
                    binding::bind_empty_namespace(&auth, &session, &keys, &id, &format!("race-{i}"))
                })
            })
            .collect::<Vec<_>>();
        let outcomes = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
        for error in outcomes.into_iter().filter_map(Result::err) {
            assert_refusal(&error, 409, "CONTENT_NAMESPACE_UNAVAILABLE");
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_commit_keeps_challenge_and_tokens_retryable() {
    let server = Server::new().await;
    let url = server.url.clone();
    let path = server.auth_path.clone();
    tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        let keys = DeviceKeypairs::generate().unwrap();
        let control = session(&auth, "fault-owner", &keys);
        let id = AccountId::generate().unwrap().to_hex();
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_binding BEFORE INSERT ON content_bindings
            WHEN NEW.state='active' BEGIN SELECT RAISE(ABORT,'injected commit failure'); END;",
        )
        .unwrap();
        let failure =
            binding::bind_empty_namespace(&auth, &control, &keys, &id, "retry-commit").unwrap_err();
        assert!(matches!(failure, pergamon_sync::SyncError::Transport(_)));
        assert!(
            binding::binding_status(&auth, &control)
                .unwrap()
                .binding
                .is_none()
        );
        conn.execute_batch("DROP TRIGGER fail_binding;").unwrap();
        let receipt =
            binding::bind_empty_namespace(&auth, &control, &keys, &id, "retry-commit").unwrap();
        assert_eq!(receipt.content_account_id, id);
        let active = binding::login_device(&auth, "fault-owner", PASSWORD, &keys).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tokens", [], |r| r.get(0))
            .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_pair BEFORE INSERT ON tokens
            WHEN NEW.kind='refresh' BEGIN SELECT RAISE(ABORT,'injected pair failure'); END;",
        )
        .unwrap();
        assert!(binding::login_device(&auth, "fault-owner", PASSWORD, &keys).is_err());
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, after, "initial mint must not leave half a pair");
        conn.execute_batch("DROP TRIGGER fail_pair;").unwrap();
        assert!(transport(&url, &active).pull(&id, 0, None).is_ok());
        conn.execute(
            "UPDATE tokens SET expires_at=0 WHERE token_id=?1",
            rusqlite::params![
                pergamon_sync::auth::token_id_from_bearer(&active.access_token).unwrap()
            ],
        )
        .unwrap();
        assert!(transport(&url, &active).pull(&id, 0, None).is_err());
    })
    .await
    .unwrap();
}
