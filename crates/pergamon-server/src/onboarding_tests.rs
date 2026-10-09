// SPDX-License-Identifier: AGPL-3.0-only

//! Real HTTP web/OPAQUE/PoP/sync interop, not rendered-heading acceptance.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::significant_drop_tightening
)]

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use opaque_ke::ServerSetup;
use pergamon_core::sync::event::{BlobManifestEntry, EntityType, Op};
use pergamon_crypto::{AccountId, AccountRootKey, DeviceKeypairs};
use pergamon_keystore::DeviceKeyStore;
use pergamon_storage::{Database, sync::FieldMap};
use pergamon_sync::RelayTransport as _;
use pergamon_sync::{
    BlobStore, CryptoContext, FsBlobStore, TransportCredential,
    account_binding::{self as binding},
    http_auth::HttpAuth,
};
use pergamon_sync_server::{
    AbuseConfig, SyncStore,
    auth::{AuthState, PergamonCipherSuite, store::AuthStore, throttle::ThrottleConfig},
};
use serde_json::json;
use tower::ServiceExt as _;

use crate::{
    auth::AdminCredentials,
    onboarding::{SyncConfig, SyncService},
    operator_session::OperatorSessions,
    state::AppState,
};

const RELAY_PASSWORD: &str = "relay-password-fixture-unique";
const KEY_PASSWORD: &str = "encrypted-key-file-fixture";
const OPERATOR_PASSWORD: &str = "local-operator-fixture";
const CONTENT: &str = "Pre-existing local article body: round-trip fixture marker";

struct Relay {
    dir: tempfile::TempDir,
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    async fn new() -> Self {
        Self::with_abuse(AbuseConfig {
            strict_rate_limit_burst: 2_000,
            ..AbuseConfig::default()
        })
        .await
    }

    async fn with_abuse(abuse: AbuseConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = pergamon_sync_server::AppState::new(
            SyncStore::open(dir.path().join("content.db")).unwrap(),
        );
        let auth = AuthState::new(
            AuthStore::open(dir.path().join("auth.db")).unwrap(),
            ServerSetup::<PergamonCipherSuite>::new(&mut rand::rngs::OsRng),
            "web-fixture",
            ThrottleConfig::default(),
        );
        let app =
            pergamon_sync_server::build_router_multitenant_hardened(state, auth.clone(), &abuse);
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
        Self { dir, url, task }
    }

    fn all_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for name in ["content.db", "auth.db"] {
            for suffix in ["", "-wal", "-shm"] {
                match std::fs::read(self.dir.path().join(format!("{name}{suffix}"))) {
                    Ok(mut data) => bytes.append(&mut data),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("reading fixture persistence: {error}"),
                }
            }
        }
        bytes
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Host {
    _dir: tempfile::TempDir,
    config: SyncConfig,
    service: Arc<SyncService>,
    state: AppState,
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Host {
    async fn new(seed: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = SyncConfig {
            db_path: dir.path().join("library.db"),
            key_file: dir.path().join("keys.json"),
            blob_dir: dir.path().join("blobs"),
            account: "default".into(),
            interval_secs: 1,
            allow_insecure_loopback: true,
        };
        let db = Database::open(&config.db_path).unwrap();
        if let Some(id) = seed {
            seed_document(&db, id);
        }
        let service = Arc::new(SyncService::new(config.clone()).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = AppState {
            db: Arc::new(Mutex::new(db)),
            http: reqwest::Client::new(),
            admin_auth: Some(AdminCredentials::new(
                "owner".into(),
                OPERATOR_PASSWORD.into(),
            )),
            sync: Some(service.clone()),
            operator: Some(Arc::new(OperatorSessions::new(&url, true).unwrap())),
        };
        let app = crate::build_router(state.clone(), None);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            _dir: dir,
            config,
            service,
            state,
            url,
            task,
        }
    }

    fn db(&self) -> Database {
        Database::open(&self.config.db_path).unwrap()
    }
    fn keys(&self) -> DeviceKeyStore {
        DeviceKeyStore::encrypted_file(&self.config.key_file, KEY_PASSWORD.as_bytes()).unwrap()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.service.shutdown().unwrap();
        self.task.abort();
    }
}

fn seed_document(db: &Database, id: &str) {
    let fields: FieldMap = serde_json::from_value(json!({
        "title":"Already local", "url":"https://article.example/fixture", "content_type":"article",
        "status":"inbox", "content_text":CONTENT,
    }))
    .unwrap();
    db.emit_change(
        EntityType::Document,
        id,
        Op::Upsert,
        fields,
        Vec::new(),
        1_000,
    )
    .unwrap();
}

struct Browser {
    client: reqwest::Client,
    origin: String,
    cookie: String,
    csrf: String,
    revision: String,
}

fn hidden(html: &str, name: &str) -> String {
    html.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .into()
}

impl Browser {
    async fn new(host: &Host) -> Self {
        let mut browser = Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            origin: host.url.clone(),
            cookie: String::new(),
            csrf: String::new(),
            revision: String::new(),
        };
        browser.page("/admin/sync-remote").await;
        browser
    }

    async fn page(&mut self, path: &str) -> String {
        let response = self
            .client
            .get(format!("{}{path}", self.origin))
            .basic_auth("owner", Some(OPERATOR_PASSWORD))
            .header("cookie", &self.cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("cache-control").unwrap(),
            "private, no-store"
        );
        assert_eq!(
            response.headers().get("referrer-policy").unwrap(),
            "strict-origin"
        );
        if let Some(cookie) = response.headers().get("set-cookie") {
            self.cookie = cookie.to_str().unwrap().split(';').next().unwrap().into();
            assert!(cookie.to_str().unwrap().contains("HttpOnly"));
            assert!(cookie.to_str().unwrap().contains("SameSite=Strict"));
        }
        let html = response.text().await.unwrap();
        self.csrf = hidden(&html, "_csrf");
        self.revision = hidden(&html, "revision");
        html
    }

    async fn post(&mut self, action: &str, fields: &[(&str, &str)]) -> (StatusCode, String) {
        let mut form = vec![
            ("_csrf", self.csrf.as_str()),
            ("revision", self.revision.as_str()),
            ("action", action),
        ];
        form.extend_from_slice(fields);
        let response = self
            .client
            .post(format!("{}/admin/sync-remote/action", self.origin))
            .basic_auth("owner", Some(OPERATOR_PASSWORD))
            .header("cookie", &self.cookie)
            .header("origin", &self.origin)
            .form(&form)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let html = response.text().await.unwrap();
        if status == StatusCode::SEE_OTHER {
            self.page("/admin/sync-remote").await;
        } else if html.contains("name=\"revision\"") {
            self.csrf = hidden(&html, "_csrf");
            self.revision = hidden(&html, "revision");
        }
        (status, html)
    }

    async fn unlock(&mut self) {
        assert_eq!(
            self.post("unlock", &[("unlock_passphrase", KEY_PASSWORD)])
                .await
                .0,
            StatusCode::SEE_OTHER
        );
    }

    async fn select(&mut self, relay: &Relay, flow: &str) {
        assert_eq!(
            self.post("select", &[("server", &relay.url), ("flow", flow)])
                .await
                .0,
            StatusCode::SEE_OTHER
        );
    }

    async fn authenticate(&mut self, register: bool) {
        let mut fields = vec![
            ("identity", "web-owner"),
            ("password", RELAY_PASSWORD),
            ("confirm_create", "yes"),
        ];
        if register {
            fields.push(("register", "yes"));
        }
        let (status, body) = self.post("authenticate", &fields).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    }
}

async fn connected(host: &Host) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let status = host.service.snapshot().unwrap();
            if status.label == "Connected" {
                break;
            }
            assert!(
                !matches!(status.label.as_str(), "Sign in again" | "Sync incomplete"),
                "{}",
                status.message
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_web_create_capture_recovery_join_and_later_mutation_round_trip() {
    let relay = Relay::new().await;
    let id = uuid::Uuid::new_v4().to_string();
    let owner = Host::new(Some(&id)).await;
    let mut browser = Browser::new(&owner).await;
    assert_eq!(owner.db().pending_outbox_count().unwrap(), 0);
    browser.unlock().await;
    browser.select(&relay, "create").await;
    browser.authenticate(true).await;

    let intent = owner.db().remote_account_binding().unwrap().unwrap();
    assert_eq!(intent.state, "pending");
    assert!(owner.db().sync_state().unwrap().server_url.is_none());
    assert_eq!(owner.db().pending_outbox_count().unwrap(), 0);
    assert_ne!(
        intent.auth_tenant_id.as_deref(),
        Some(intent.content_account_id.as_str())
    );
    let capture_page = browser.page("/admin/sync-remote/recovery").await;
    let code = capture_page
        .split("spellcheck=\"false\">")
        .nth(1)
        .unwrap()
        .split("</textarea>")
        .next()
        .unwrap()
        .to_owned();
    assert!(!code.is_empty());
    assert_eq!(
        browser.post("acknowledge", &[]).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        owner.db().remote_account_binding().unwrap().unwrap().state,
        "pending"
    );
    assert_eq!(
        browser
            .post("acknowledge", &[("confirm_recovery", "yes")])
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    connected(&owner).await;
    assert_eq!(owner.db().pending_outbox_count().unwrap(), 0);
    assert!(
        owner
            .keys()
            .load_bootstrap_recovery("default", &relay.url)
            .unwrap()
            .is_none()
    );

    let fresh = Host::new(None).await;
    let mut joining = Browser::new(&fresh).await;
    joining.unlock().await;
    joining.select(&relay, "join").await;
    joining.authenticate(false).await;
    assert!(
        fresh.keys().load_ark("default").unwrap().is_none(),
        "password-only login must not invent an ARK"
    );
    assert_eq!(fresh.db().count_content_items(None).unwrap(), 0);
    assert_eq!(
        joining
            .post("recover", &[("recovery_code", "incorrect-fixture-code")])
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(fresh.keys().load_ark("default").unwrap().is_none());
    assert_eq!(
        joining.post("recover", &[("recovery_code", &code)]).await.0,
        StatusCode::SEE_OTHER
    );
    connected(&fresh).await;
    let peer_doc = fresh
        .db()
        .read_entity_fields(EntityType::Document, &id)
        .unwrap()
        .unwrap();
    assert_eq!(peer_doc["content_text"], json!(CONTENT));
    assert_eq!(
        owner
            .keys()
            .load_ark("default")
            .unwrap()
            .unwrap()
            .expose_bytes(),
        fresh
            .keys()
            .load_ark("default")
            .unwrap()
            .unwrap()
            .expose_bytes()
    );

    let response = browser
        .client
        .patch(format!("{}/api/items/{id}", owner.url))
        .json(&json!({"status":"archived"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if fresh
                .db()
                .read_entity_fields(EntityType::Document, &id)
                .unwrap()
                .unwrap()["status"]
                == json!("archived")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();

    let ark = *owner
        .keys()
        .load_ark("default")
        .unwrap()
        .unwrap()
        .expose_bytes();
    let raw = relay.all_bytes();
    assert!(!raw.windows(CONTENT.len()).any(|w| w == CONTENT.as_bytes()));
    assert!(!raw.windows(ark.len()).any(|w| w == ark));
    assert!(
        !raw.windows(RELAY_PASSWORD.len())
            .any(|w| w == RELAY_PASSWORD.as_bytes())
    );
    assert!(!raw.windows(code.len()).any(|w| w == code.as_bytes()));
    let key_file = std::fs::read(&owner.config.key_file).unwrap();
    assert!(!key_file.windows(ark.len()).any(|w| w == ark));
    let setup_text = format!("{:?}", owner.db().web_sync_setup().unwrap());
    assert!(!setup_text.contains(RELAY_PASSWORD));
    assert!(!setup_text.contains(&code));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_local_account_required_blob_round_trips_without_rekeying() {
    let relay = Relay::new().await;
    let host = Host::new(None).await;
    let paths = host.config.clone();
    let seeded = tokio::task::spawn_blocking(move || {
        let keys = DeviceKeypairs::generate().unwrap();
        let id = AccountId::from_bytes([3; 16]);
        let ark = AccountRootKey::from_bytes([7; 32]);
        let mut store = DeviceKeyStore::encrypted_file(&paths.key_file, KEY_PASSWORD.as_bytes()).unwrap();
        store.save_account_material("default", &keys, &ark, &id).unwrap();
        let db = Database::open(&paths.db_path).unwrap();
        db.set_sync_identity(&id.to_hex(), keys.device_id(), 0, None).unwrap();
        let crypto = CryptoContext::new(AccountRootKey::from_bytes([7; 32]), id.to_hex(), keys.device_id().into(), *keys.ed25519_signing(), 0).unwrap();
        let payload = b"required durable raw-html fixture blob".to_vec();
        let encrypted = crypto.encrypt_blob_plaintext(&payload).unwrap();
        let hash = pergamon_crypto::primitives::blake3_hash(&payload).iter().fold(String::new(), |mut text, b| {
            write!(&mut text, "{b:02x}").unwrap();
            text
        });
        FsBlobStore::new(&paths.blob_dir).unwrap().store(&hash, &payload).unwrap();
        let document = uuid::Uuid::new_v4().to_string();
        let fields = serde_json::from_value(json!({"title":"Existing blob article","content_type":"article","status":"inbox","content_text":CONTENT})).unwrap();
        db.emit_change(EntityType::Document, &document, Op::Upsert, fields,
            vec![BlobManifestEntry { ct_hash: encrypted.ct_hash.clone(), plaintext_hash: hash.clone(), role:"raw_html".into(),
                plaintext_len: u64::try_from(payload.len()).unwrap() }], 1_000).unwrap();
        (id, *keys.ed25519_signing(), document, hash, payload, encrypted.ciphertext)
    }).await.unwrap();
    let mut browser = Browser::new(&host).await;
    browser.unlock().await;
    browser.select(&relay, "attach").await;
    browser.authenticate(true).await;
    browser.page("/admin/sync-remote/recovery").await;
    assert_eq!(
        browser
            .post("acknowledge", &[("confirm_recovery", "yes")])
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    connected(&host).await;
    assert_eq!(
        host.keys().load_account_id("default").unwrap().unwrap(),
        seeded.0
    );
    assert_eq!(
        *host
            .keys()
            .load_device_keys("default")
            .unwrap()
            .unwrap()
            .ed25519_signing(),
        seeded.1
    );
    assert_eq!(
        *host
            .keys()
            .load_ark("default")
            .unwrap()
            .unwrap()
            .expose_bytes(),
        [7; 32]
    );

    let capture_code = tokio::task::spawn_blocking({
        let url = relay.url.clone();
        move || {
            let keys = DeviceKeypairs::generate().unwrap();
            let auth = HttpAuth::new(&url).unwrap();
            let session =
                binding::login_device(&auth, "web-owner", RELAY_PASSWORD.as_bytes(), &keys)
                    .unwrap();
            (keys, session)
        }
    })
    .await
    .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let url = relay.url.clone();
    let content = seeded.0.clone();
    let document = seeded.2.clone();
    let target = destination.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let (keys, session) = capture_code;
        let credential = Some(TransportCredential::Bearer {
            token: session.access_token,
        });
        let transport =
            pergamon_sync::http::HttpTransport::with_credential(&url, credential.clone()).unwrap();
        let relay = pergamon_sync::HttpRelay::with_credential(&url, credential).unwrap();
        let roster = pergamon_sync::onboarding::roster(&relay, &content).unwrap();
        let crypto = CryptoContext::new(
            AccountRootKey::from_bytes([7; 32]),
            content.to_hex(),
            keys.device_id().into(),
            *keys.ed25519_signing(),
            0,
        )
        .unwrap();
        let db = Database::open(&target.join("peer.db")).unwrap();
        db.set_sync_identity(&content.to_hex(), keys.device_id(), 0, Some(&url))
            .unwrap();
        let blobs = FsBlobStore::new(target.join("blobs")).unwrap();
        let engine = pergamon_sync::SyncEngine::new(
            transport,
            crypto,
            pergamon_sync::DeviceKeyDirectory::from_roster(&roster),
        );
        assert!(engine.pull(&db, &blobs).unwrap() > 0);
        assert_eq!(
            db.read_entity_fields(EntityType::Document, &document)
                .unwrap()
                .unwrap()["content_text"],
            json!(CONTENT)
        );
    })
    .await
    .unwrap();
    let reopened = FsBlobStore::new(destination.path().join("blobs")).unwrap();
    assert_eq!(reopened.load(&seeded.3).unwrap().unwrap(), seeded.4);
    let raw = relay.all_bytes();
    assert!(
        raw.windows(seeded.5.len())
            .any(|w| w == seeded.5.as_slice()),
        "ciphertext positive control includes WAL"
    );
    assert!(
        !raw.windows(seeded.4.len())
            .any(|w| w == seeded.4.as_slice())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_csrf_origin_and_local_use_are_independent() {
    let host = Host::new(None).await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{}/inbox", host.url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .get(format!("{}/admin/sync-remote", host.url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut browser = Browser::new(&host).await;
    let missing = browser
        .client
        .post(format!("{}/admin/sync-remote/action", host.url))
        .basic_auth("owner", Some(OPERATOR_PASSWORD))
        .header("cookie", &browser.cookie)
        .header("origin", &host.url)
        .form(&[
            ("action", "unlock"),
            ("revision", "0"),
            ("unlock_passphrase", KEY_PASSWORD),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);
    let foreign = browser
        .client
        .post(format!("{}/admin/sync-remote/action", host.url))
        .basic_auth("owner", Some(OPERATOR_PASSWORD))
        .header("cookie", &browser.cookie)
        .header("origin", "https://foreign.example")
        .form(&[
            ("_csrf", browser.csrf.as_str()),
            ("action", "unlock"),
            ("revision", "0"),
            ("unlock_passphrase", KEY_PASSWORD),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    assert!(!host.service.snapshot().unwrap().unlocked);
    browser.unlock().await;
    assert_eq!(
        browser
            .post(
                "select",
                &[("server", "file:///tmp/private"), ("flow", "create")]
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        browser
            .post(
                "select",
                &[
                    ("server", "https://owner:secret@relay.example"),
                    ("flow", "create")
                ]
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(host.db().remote_account_binding().unwrap().is_none());
    let router = crate::build_router(
        AppState {
            admin_auth: None,
            ..host.state.clone()
        },
        None,
    );
    let response = router
        .oneshot(
            axum::http::Request::get("/admin/sync-remote")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_relay_and_cancel_do_not_bind_or_discard_local_data() {
    let id = uuid::Uuid::new_v4().to_string();
    let host = Host::new(Some(&id)).await;
    let mut browser = Browser::new(&host).await;
    browser.unlock().await;
    let blind = axum::Router::new().route("/health", axum::routing::get(|| async { "healthy" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, blind).await.unwrap();
    });
    assert_eq!(
        browser
            .post("select", &[("server", &url), ("flow", "create")])
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = browser
        .post(
            "authenticate",
            &[
                ("identity", "web-owner"),
                ("password", RELAY_PASSWORD),
                ("register", "yes"),
                ("confirm_create", "yes"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains(&format!("value=\"{RELAY_PASSWORD}\"")));
    assert!(host.db().remote_account_binding().unwrap().is_none());
    assert!(host.keys().load_ark("default").unwrap().is_none());
    assert_eq!(browser.post("cancel", &[]).await.0, StatusCode::SEE_OTHER);
    assert_eq!(
        host.db()
            .read_entity_fields(EntityType::Document, &id)
            .unwrap()
            .unwrap()["content_text"],
        json!(CONTENT)
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sas_join_requires_real_approval_and_retries_do_not_duplicate_publications() {
    let relay = Relay::new().await;
    let id = uuid::Uuid::new_v4().to_string();
    let owner = Host::new(Some(&id)).await;
    let mut approving = Browser::new(&owner).await;
    approving.unlock().await;
    approving.select(&relay, "create").await;
    approving.authenticate(true).await;
    approving.page("/admin/sync-remote/recovery").await;
    assert_eq!(
        approving
            .post("acknowledge", &[("confirm_recovery", "yes")])
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    connected(&owner).await;
    let owner_id = owner
        .keys()
        .load_device_keys("default")
        .unwrap()
        .unwrap()
        .device_id()
        .to_owned();

    let fresh = Host::new(None).await;
    let mut joining = Browser::new(&fresh).await;
    joining.unlock().await;
    joining.select(&relay, "join").await;
    joining.authenticate(false).await;
    assert_eq!(
        joining.post("enroll", &[("device", &owner_id)]).await.0,
        StatusCode::SEE_OTHER
    );
    let new_id = fresh
        .keys()
        .load_device_keys("default")
        .unwrap()
        .unwrap()
        .device_id()
        .to_owned();
    let join_sas = fresh.service.snapshot().unwrap().sas;
    assert_eq!(
        approving.post("sas", &[("device", &new_id)]).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(join_sas, owner.service.snapshot().unwrap().sas);
    assert_eq!(
        approving
            .post(
                "approve",
                &[("device", &new_id), ("expect_sas", "incorrect")]
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(fresh.keys().load_ark("default").unwrap().is_none());
    assert_eq!(
        joining.post("accept", &[("expect_sas", &join_sas)]).await.0,
        StatusCode::BAD_REQUEST
    );
    assert!(fresh.keys().load_ark("default").unwrap().is_none());
    assert_eq!(
        approving
            .post("approve", &[("device", &new_id), ("expect_sas", &join_sas)])
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        approving
            .post("approve", &[("device", &new_id), ("expect_sas", &join_sas)])
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        joining
            .post("accept", &[("expect_sas", "different")])
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(fresh.keys().load_ark("default").unwrap().is_none());
    assert_eq!(
        joining.post("accept", &[("expect_sas", &join_sas)]).await.0,
        StatusCode::SEE_OTHER
    );
    connected(&fresh).await;
    assert_eq!(
        fresh
            .db()
            .read_entity_fields(EntityType::Document, &id)
            .unwrap()
            .unwrap()["content_text"],
        json!(CONTENT)
    );
    let url = relay.url.clone();
    let paths = owner.config.clone();
    tokio::task::spawn_blocking(move || {
        let store =
            DeviceKeyStore::encrypted_file(&paths.key_file, KEY_PASSWORD.as_bytes()).unwrap();
        let bytes = store.load_remote_session("default", &url).unwrap().unwrap();
        let session: binding::RemoteSession = serde_json::from_slice(&bytes).unwrap();
        let relay = pergamon_sync::HttpRelay::with_credential(
            &url,
            Some(TransportCredential::Bearer {
                token: session.access_token,
            }),
        )
        .unwrap();
        assert_eq!(
            relay
                .wraps_list(&session.content_account_id.unwrap(), &new_id, 0)
                .unwrap()
                .len(),
            1
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_access_refreshes_over_http_and_revocation_is_latched_across_restart() {
    let relay = Relay::new().await;
    let host = Host::new(None).await;
    let mut browser = Browser::new(&host).await;
    browser.unlock().await;
    browser.select(&relay, "create").await;
    browser.authenticate(true).await;
    browser.page("/admin/sync-remote/recovery").await;
    assert_eq!(
        browser
            .post("acknowledge", &[("confirm_recovery", "yes")])
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    connected(&host).await;
    let before: binding::RemoteSession = serde_json::from_slice(
        &host
            .keys()
            .load_remote_session("default", &relay.url)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let auth_db = rusqlite::Connection::open(relay.dir.path().join("auth.db")).unwrap();
    auth_db.busy_timeout(Duration::from_secs(5)).unwrap();
    auth_db
        .execute(
            "UPDATE tokens SET expires_at=0 WHERE kind='access' AND revoked_at IS NULL",
            [],
        )
        .unwrap();
    drop(auth_db);
    let expired = browser
        .client
        .get(format!("{}/v1/events", relay.url))
        .query(&[
            ("account_id", before.content_account_id.as_deref().unwrap()),
            ("after", "0"),
        ])
        .bearer_auth(&before.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(browser.post("trigger", &[]).await.0, StatusCode::SEE_OTHER);
    let rotated = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let current: binding::RemoteSession = serde_json::from_slice(
                &host
                    .keys()
                    .load_remote_session("default", &relay.url)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            if current.refresh_token != before.refresh_token {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(before.content_account_id, rotated.content_account_id);
    assert_eq!(before.auth_tenant_id, rotated.auth_tenant_id);
    assert_eq!(before.binding_version, rotated.binding_version);
    let url = relay.url.clone();
    let paths = host.config.clone();
    tokio::task::spawn_blocking(move || {
        let store =
            DeviceKeyStore::encrypted_file(&paths.key_file, KEY_PASSWORD.as_bytes()).unwrap();
        let keys = store.load_device_keys("default").unwrap().unwrap();
        let auth = HttpAuth::new(&url).unwrap();
        assert!(
            binding::refresh_session(&auth, &keys, &before).is_err(),
            "consumed refresh token must be rejected"
        );
        binding::revoke_device_session(&auth, &rotated, keys.device_id()).unwrap();
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        while !host.db().web_sync_needs_login().unwrap() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(host.service.snapshot().unwrap().needs_login);
    assert_eq!(browser.post("start", &[]).await.0, StatusCode::CONFLICT);
    let config = host.config.clone();
    let original = host.service.clone();
    tokio::task::spawn_blocking(move || {
        original.shutdown().unwrap();
        let restarted = SyncService::new(config).unwrap();
        assert_eq!(restarted.snapshot().unwrap().label, "Locked");
        assert!(!restarted.snapshot().unwrap().unlocked);
        assert!(restarted.snapshot().unwrap().needs_login);
        let mut command = crate::onboarding::SyncCommand::new(
            "unlock",
            restarted.snapshot().unwrap().setup.unwrap().revision,
        );
        command.unlock_passphrase = Some(KEY_PASSWORD.into());
        assert!(
            restarted.execute(&command).is_err(),
            "deployment unlock must not bypass required fresh login"
        );
        restarted.shutdown().unwrap();
    })
    .await
    .unwrap();
    assert_eq!(
        browser
            .post(
                "renew",
                &[("identity", "web-owner"), ("password", RELAY_PASSWORD)]
            )
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    connected(&host).await;
    assert!(!host.db().web_sync_needs_login().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_capture_cancel_retry_keeps_keys_receipt_and_recovery_artifact() {
    let relay = Relay::new().await;
    let document = uuid::Uuid::new_v4().to_string();
    let host = Host::new(Some(&document)).await;
    let mut browser = Browser::new(&host).await;
    browser.unlock().await;
    browser.select(&relay, "create").await;
    browser.authenticate(true).await;
    let intent = host.db().remote_account_binding().unwrap().unwrap();
    let ark = *host
        .keys()
        .load_ark("default")
        .unwrap()
        .unwrap()
        .expose_bytes();
    let capture = host
        .keys()
        .load_bootstrap_recovery("default", &relay.url)
        .unwrap()
        .unwrap();
    assert_eq!(browser.post("cancel", &[]).await.0, StatusCode::SEE_OTHER);
    assert_eq!(host.db().pending_outbox_count().unwrap(), 0);
    assert_eq!(
        *host
            .keys()
            .load_ark("default")
            .unwrap()
            .unwrap()
            .expose_bytes(),
        ark
    );
    browser.authenticate(true).await;
    assert_eq!(host.db().remote_account_binding().unwrap().unwrap(), intent);
    assert_eq!(
        host.keys()
            .load_bootstrap_recovery("default", &relay.url)
            .unwrap()
            .unwrap(),
        capture
    );
    let config = host.config.clone();
    let original = host.service.clone();
    tokio::task::spawn_blocking(move || {
        original.shutdown().unwrap();
        let restarted = SyncService::new(config).unwrap();
        assert_eq!(restarted.snapshot().unwrap().label, "Locked");
        assert!(!restarted.snapshot().unwrap().unlocked);
        let revision = restarted.snapshot().unwrap().setup.unwrap().revision;
        let mut unlock = crate::onboarding::SyncCommand::new("unlock", revision);
        unlock.unlock_passphrase = Some(KEY_PASSWORD.into());
        restarted.execute(&unlock).unwrap();
        let material = restarted.recovery_capture().unwrap();
        assert_eq!(serde_json::to_vec(&material).unwrap(), capture);
        assert_eq!(
            restarted.snapshot().unwrap().setup.unwrap().phase,
            "recovery"
        );
        restarted.shutdown().unwrap();
    })
    .await
    .unwrap();
    assert_eq!(
        host.db()
            .read_entity_fields(EntityType::Document, &document)
            .unwrap()
            .unwrap()["content_text"],
        json!(CONTENT)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_relay_rate_limit_and_wrong_password_are_not_connected_states() {
    let limited = Relay::with_abuse(AbuseConfig {
        strict_rate_limit_rps: 1,
        strict_rate_limit_burst: 1,
        ..AbuseConfig::default()
    })
    .await;
    let host = Host::new(None).await;
    let mut browser = Browser::new(&host).await;
    browser.unlock().await;
    browser.select(&limited, "create").await;
    let (status, body) = browser
        .post(
            "authenticate",
            &[
                ("identity", "rate-fixture"),
                ("password", RELAY_PASSWORD),
                ("register", "yes"),
                ("confirm_create", "yes"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body.contains("Too many requests"));
    assert!(host.service.snapshot().unwrap().last_success.is_empty());
    assert!(host.db().sync_state().unwrap().server_url.is_none());
    let relay = Relay::new().await;
    let url = relay.url.clone();
    tokio::task::spawn_blocking(move || {
        let auth = HttpAuth::new(&url).unwrap();
        binding::register_identity(&auth, "password-fixture", RELAY_PASSWORD.as_bytes()).unwrap();
    })
    .await
    .unwrap();
    let wrong = Host::new(None).await;
    let mut login = Browser::new(&wrong).await;
    login.unlock().await;
    login.select(&relay, "join").await;
    let (status, body) = login
        .post(
            "authenticate",
            &[
                ("identity", "password-fixture"),
                ("password", "wrong-password"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Authentication failed"));
    assert!(!body.contains("value=\"wrong-password\""));
    assert!(wrong.keys().load_ark("default").unwrap().is_none());
    assert!(wrong.db().sync_state().unwrap().server_url.is_none());
}
