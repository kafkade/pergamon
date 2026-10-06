// SPDX-License-Identifier: AGPL-3.0-only

//! Explicit process-level acceptance gate; requires a built Apache CLI.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use opaque_ke::ServerSetup;
use pergamon_crypto::{AccountId, AccountRootKey, DeviceKeypairs};
use pergamon_keystore::DeviceKeyStore;
use pergamon_storage::Database;
use pergamon_sync::{
    CryptoContext, DeviceKeyDirectory, MemoryBlobStore, SyncEngine, TransportCredential,
    account_binding as binding, http_auth::HttpAuth,
};
use pergamon_sync_server::{
    AbuseConfig, AppState, SyncStore,
    auth::{AuthState, PergamonCipherSuite, store::AuthStore, throttle::ThrottleConfig},
    build_router_multitenant_hardened,
};
use rand::rngs::OsRng;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const PASSWORD: &str = "cli-pake-fixture-password-never-persist";
const KEY_PASSWORD: &[u8] = b"local-keyfile-fixture-password";

struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pergamon-cli-binding-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self { path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run(binary: &Path, home: &Path, args: &[&str], recovery: &str) -> std::process::Output {
    Command::new(binary)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("PERGAMON_DB", home.join("library.db"))
        .env("PERGAMON_DATA_DIR", home)
        .env(
            "PERGAMON_KEY_PASSPHRASE",
            std::str::from_utf8(KEY_PASSWORD).unwrap(),
        )
        .env("PERGAMON_SYNC_AUTH_PASSWORD", PASSWORD)
        .env("PERGAMON_RECOVERY_PASSPHRASE", recovery)
        .env_remove("PERGAMON_SYNC_BEARER_TOKEN")
        .env_remove("PERGAMON_SYNC_BASIC_USER")
        .env_remove("PERGAMON_SYNC_BASIC_PASSWORD")
        .output()
        .expect("run built CLI")
}

fn success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(PASSWORD));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(PASSWORD));
}

async fn server(home: &Path) -> (String, tokio::task::JoinHandle<()>) {
    let state = AppState::new(SyncStore::open(home.join("content.db")).unwrap());
    let auth = AuthState::new(
        AuthStore::open(home.join("auth.db")).unwrap(),
        ServerSetup::<PergamonCipherSuite>::new(&mut OsRng),
        "fixture",
        ThrottleConfig::default(),
    );
    let app = build_router_multitenant_hardened(
        state,
        auth,
        &AbuseConfig {
            strict_rate_limit_burst: 2000,
            ..AbuseConfig::default()
        },
    );
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
    (url, task)
}

fn binary() -> PathBuf {
    let path = std::env::var_os("PERGAMON_TEST_CLI")
        .expect("build the workspace, then set PERGAMON_TEST_CLI to the built pergamon binary");
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
    };
    std::fs::canonicalize(path).expect("PERGAMON_TEST_CLI must name the built CLI")
}

fn seed_local_library(
    home: &Path,
    ark: &AccountRootKey,
    keys: &DeviceKeypairs,
    id: Option<&AccountId>,
) -> PathBuf {
    let keyfile = home.join("keys.json");
    let mut store = DeviceKeyStore::encrypted_file(&keyfile, KEY_PASSWORD).unwrap();
    store.save_device_keys("default", keys).unwrap();
    store.save_ark("default", ark).unwrap();
    if let Some(id) = id {
        store.save_account_id("default", id).unwrap();
    }
    let db = Database::open(&home.join("library.db")).unwrap();
    if let Some(id) = id {
        db.set_sync_identity(&id.to_hex(), keys.device_id(), 0, None)
            .unwrap();
    }
    db.emit_change(
        pergamon_core::sync::event::EntityType::Document,
        &uuid::Uuid::new_v4().to_string(),
        pergamon_core::sync::event::Op::Upsert,
        json!({
            "url":"https://regression.example/local","title":"Local library must not be adopted",
            "content_type":"article","status":"inbox","content_text":"Local content"
        })
        .as_object()
        .unwrap()
        .clone(),
        vec![],
        1_700_000_000_000,
    )
    .unwrap();
    keyfile
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit PERGAMON_TEST_CLI built-binary acceptance gate"]
async fn cli_authenticated_binding_cannot_fall_back_to_another_blind_relay() {
    let binary = binary();
    let authenticated_home = Fixture::new();
    let blind_home = Fixture::new();
    let (a, a_task) = server(&authenticated_home.path).await;
    let blind_state = AppState::new(SyncStore::open(blind_home.path.join("content.db")).unwrap());
    let blind_store = blind_state.store.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let b = format!("http://{}", listener.local_addr().unwrap());
    let b_task = tokio::spawn(async move {
        axum::serve(listener, pergamon_sync_server::build_router(blind_state))
            .await
            .unwrap();
    });
    tokio::task::spawn_blocking(move || {
        let local = Fixture::new();
        let id = AccountId::generate().unwrap();
        let ark = AccountRootKey::from_bytes([101; 32]);
        let keys = DeviceKeypairs::generate().unwrap();
        let keyfile = seed_local_library(&local.path, &ark, &keys, Some(&id));
        success(&run(
            &binary,
            &local.path,
            &[
                "sync-remote",
                "enable",
                "--server",
                &a,
                "--relay-identity",
                "fallback-owner",
                "--register-relay-account",
                "--key-file",
                keyfile.to_str().unwrap(),
            ],
            "recovery",
        ));
        let db = Database::open(&local.path.join("library.db")).unwrap();
        let binding = db.remote_account_binding().unwrap().unwrap();
        let before = db.sync_state().unwrap();
        let outbox = db.pending_outbox_count().unwrap();
        let key_bytes = std::fs::read(&keyfile).unwrap();
        for target in [&a, &b] {
            let failure = run(
                &binary,
                &local.path,
                &[
                    "sync-remote",
                    "enable",
                    "--server",
                    target,
                    "--key-file",
                    keyfile.to_str().unwrap(),
                ],
                "recovery",
            );
            assert!(
                !failure.status.success(),
                "legacy enable must not alter an authenticated binding"
            );
            assert_eq!(db.sync_state().unwrap().server_url, before.server_url);
            assert_eq!(db.remote_account_binding().unwrap().unwrap(), binding);
            assert_eq!(db.pending_outbox_count().unwrap(), outbox);
            assert_eq!(std::fs::read(&keyfile).unwrap(), key_bytes);
        }
        // Emulate a stale/incorrect saved URL to exercise central admission,
        // independently of the command-dispatch guard.
        db.set_sync_identity(&id.to_hex(), keys.device_id(), before.key_epoch, Some(&b))
            .unwrap();
        let failure = run(
            &binary,
            &local.path,
            &[
                "sync-remote",
                "sync",
                "--key-file",
                keyfile.to_str().unwrap(),
            ],
            "recovery",
        );
        assert!(
            !failure.status.success(),
            "central admission must not return blind credentials for B"
        );
        assert!(String::from_utf8_lossy(&failure.stderr).contains("authenticated relay"));
        assert_eq!(db.remote_account_binding().unwrap().unwrap(), binding);
        assert_eq!(db.pending_outbox_count().unwrap(), outbox);
        assert_eq!(std::fs::read(&keyfile).unwrap(), key_bytes);
        assert_eq!(
            blind_store
                .account_usage(&id.to_hex())
                .unwrap()
                .total_objects(),
            0
        );
        db.set_sync_identity(&id.to_hex(), keys.device_id(), before.key_epoch, Some(&a))
            .unwrap();

        let empty_keyfile = local.path.join("same-identity-without-session.json");
        {
            let mut store = DeviceKeyStore::encrypted_file(&empty_keyfile, KEY_PASSWORD).unwrap();
            store.save_device_keys("default", &keys).unwrap();
            store.save_ark("default", &ark).unwrap();
            store.save_account_id("default", &id).unwrap();
        }
        let failure = run(
            &binary,
            &local.path,
            &[
                "sync-remote",
                "sync",
                "--key-file",
                empty_keyfile.to_str().unwrap(),
            ],
            "recovery",
        );
        assert!(
            !failure.status.success(),
            "an authenticated intent cannot use a missing secure session"
        );
        assert_eq!(db.sync_state().unwrap().server_url, before.server_url);
        assert_eq!(db.remote_account_binding().unwrap().unwrap(), binding);

        let pending_home = Fixture::new();
        let pending_id = AccountId::generate().unwrap();
        let pending_keys = DeviceKeypairs::generate().unwrap();
        let pending_keyfile =
            seed_local_library(&pending_home.path, &ark, &pending_keys, Some(&pending_id));
        let pending_db = Database::open(&pending_home.path.join("library.db")).unwrap();
        let intent = pending_db
            .begin_remote_binding(
                &a,
                "default",
                &pending_id.to_hex(),
                pending_keys.device_id(),
                "pending",
                "attach",
            )
            .unwrap();
        let before = pending_db.sync_state().unwrap();
        let outbox = pending_db.pending_outbox_count().unwrap();
        let key_bytes = std::fs::read(&pending_keyfile).unwrap();
        for target in [&a, &b] {
            let failure = run(
                &binary,
                &pending_home.path,
                &[
                    "sync-remote",
                    "enable",
                    "--server",
                    target,
                    "--key-file",
                    pending_keyfile.to_str().unwrap(),
                ],
                "recovery",
            );
            assert!(
                !failure.status.success(),
                "legacy enable cannot complete pending authenticated adoption"
            );
        }
        assert_eq!(
            pending_db.sync_state().unwrap().server_url,
            before.server_url
        );
        assert_eq!(
            pending_db.remote_account_binding().unwrap().unwrap(),
            intent
        );
        assert_eq!(pending_db.pending_outbox_count().unwrap(), outbox);
        assert_eq!(std::fs::read(&pending_keyfile).unwrap(), key_bytes);

        let legacy_home = Fixture::new();
        let legacy_id = AccountId::generate().unwrap();
        let legacy_keys = DeviceKeypairs::generate().unwrap();
        let legacy_keyfile =
            seed_local_library(&legacy_home.path, &ark, &legacy_keys, Some(&legacy_id));
        success(&run(
            &binary,
            &legacy_home.path,
            &[
                "sync-remote",
                "enable",
                "--server",
                &b,
                "--key-file",
                legacy_keyfile.to_str().unwrap(),
            ],
            "recovery",
        ));
        success(&run(
            &binary,
            &legacy_home.path,
            &[
                "sync-remote",
                "sync",
                "--key-file",
                legacy_keyfile.to_str().unwrap(),
            ],
            "recovery",
        ));
        assert!(
            blind_store
                .account_usage(&legacy_id.to_hex())
                .unwrap()
                .event_count
                > 0
        );
    })
    .await
    .unwrap();
    a_task.abort();
    b_task.abort();
    let _ = a_task.await;
    let _ = b_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit PERGAMON_TEST_CLI built-binary acceptance gate"]
async fn cli_non_join_login_cannot_adopt_library_or_activate_wrong_root_keys() {
    let binary = binary();
    let remote_home = Fixture::new();
    let (url, task) = server(&remote_home.path).await;
    tokio::task::spawn_blocking(move || {
        let owner = Fixture::new();
        let owner_keyfile = owner.path.join("keys.json");
        success(&run(
            &binary,
            &owner.path,
            &[
                "sync-device",
                "bootstrap",
                "--server",
                &url,
                "--relay-identity",
                "login-owner",
                "--register-relay-account",
                "--no-recovery-code",
                "--key-file",
                owner_keyfile.to_str().unwrap(),
            ],
            "recovery",
        ));
        let owner_store = DeviceKeyStore::encrypted_file(&owner_keyfile, KEY_PASSWORD).unwrap();
        let id = owner_store.load_account_id("default").unwrap().unwrap();
        let actual_ark = owner_store.load_ark("default").unwrap().unwrap();
        let owner_keys = owner_store.load_device_keys("default").unwrap().unwrap();
        let owner_db = Database::open(&owner.path.join("library.db")).unwrap();
        owner_db
            .emit_change(
                pergamon_core::sync::event::EntityType::Document,
                &uuid::Uuid::new_v4().to_string(),
                pergamon_core::sync::event::Op::Upsert,
                json!({"url":"https://remote.example/existing",
                "title":"Existing encrypted account","content_type":"article","status":"inbox"})
                .as_object()
                .unwrap()
                .clone(),
                vec![],
                1_700_000_000_000,
            )
            .unwrap();
        success(&run(
            &binary,
            &owner.path,
            &[
                "sync-remote",
                "sync",
                "--key-file",
                owner_keyfile.to_str().unwrap(),
            ],
            "recovery",
        ));

        let unbound = Fixture::new();
        let local_keys = DeviceKeypairs::generate().unwrap();
        let wrong_ark = AccountRootKey::from_bytes([103; 32]);
        let unbound_keyfile = seed_local_library(&unbound.path, &wrong_ark, &local_keys, None);
        let db = Database::open(&unbound.path.join("library.db")).unwrap();
        let outbox = db.pending_outbox_count().unwrap();
        let key_bytes = std::fs::read(&unbound_keyfile).unwrap();
        let failure = run(
            &binary,
            &unbound.path,
            &[
                "sync-remote",
                "login",
                "--server",
                &url,
                "--relay-identity",
                "login-owner",
                "--key-file",
                unbound_keyfile.to_str().unwrap(),
            ],
            "recovery",
        );
        assert!(
            !failure.status.success(),
            "non-join login cannot adopt a remote ID for an unrelated local ARK/library"
        );
        assert!(db.sync_state().unwrap().server_url.is_none());
        assert!(db.sync_state().unwrap().account_id.is_none());
        assert!(db.remote_account_binding().unwrap().is_none());
        assert_eq!(db.pending_outbox_count().unwrap(), outbox);
        assert_eq!(std::fs::read(&unbound_keyfile).unwrap(), key_bytes);

        let wrong = Fixture::new();
        let wrong_keys = DeviceKeypairs::generate().unwrap();
        let wrong_keyfile = seed_local_library(&wrong.path, &wrong_ark, &wrong_keys, Some(&id));
        let db = Database::open(&wrong.path.join("library.db")).unwrap();
        let intent = db
            .begin_remote_binding(
                &url,
                "default",
                &id.to_hex(),
                wrong_keys.device_id(),
                "pending-wrong-ark",
                "attach",
            )
            .unwrap();
        let before = db.sync_state().unwrap();
        let outbox = db.pending_outbox_count().unwrap();
        let key_bytes = std::fs::read(&wrong_keyfile).unwrap();
        let failure = run(
            &binary,
            &wrong.path,
            &[
                "sync-remote",
                "login",
                "--server",
                &url,
                "--relay-identity",
                "login-owner",
                "--key-file",
                wrong_keyfile.to_str().unwrap(),
            ],
            "recovery",
        );
        assert!(
            !failure.status.success(),
            "pending matching ID must prove existing content with its local keys"
        );
        assert!(
            String::from_utf8_lossy(&failure.stderr).contains("decrypt existing relay content")
        );
        assert_eq!(db.sync_state().unwrap().server_url, before.server_url);
        assert_eq!(db.remote_account_binding().unwrap().unwrap(), intent);
        assert_eq!(db.pending_outbox_count().unwrap(), outbox);
        assert_eq!(std::fs::read(&wrong_keyfile).unwrap(), key_bytes);

        let matching = Fixture::new();
        let matching_keyfile =
            seed_local_library(&matching.path, &actual_ark, &owner_keys, Some(&id));
        let db = Database::open(&matching.path.join("library.db")).unwrap();
        db.begin_remote_binding(
            &url,
            "default",
            &id.to_hex(),
            owner_keys.device_id(),
            "pending-correct-ark",
            "attach",
        )
        .unwrap();
        success(&run(
            &binary,
            &matching.path,
            &[
                "sync-remote",
                "login",
                "--server",
                &url,
                "--relay-identity",
                "login-owner",
                "--key-file",
                matching_keyfile.to_str().unwrap(),
            ],
            "recovery",
        ));
        assert_eq!(
            db.remote_account_binding().unwrap().unwrap().state,
            "active"
        );
        let before = db.remote_account_binding().unwrap().unwrap();
        success(&run(
            &binary,
            &matching.path,
            &[
                "sync-remote",
                "login",
                "--server",
                &url,
                "--relay-identity",
                "login-owner",
                "--key-file",
                matching_keyfile.to_str().unwrap(),
            ],
            "recovery",
        ));
        assert_eq!(db.remote_account_binding().unwrap().unwrap(), before);
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit PERGAMON_TEST_CLI built-binary acceptance gate"]
async fn cli_attaches_existing_library_and_refreshes_without_changing_keys() {
    let binary = binary();
    let server_home = Fixture::new();
    let (url, task) = server(&server_home.path).await;
    tokio::task::spawn_blocking(move || {
        let local=Fixture::new();
        let keyfile=local.path.join("keys.json");
        let id=AccountId::generate().unwrap();
        let ark=AccountRootKey::from_bytes([79;32]);
        let keys=DeviceKeypairs::generate().unwrap();
        let mut store=DeviceKeyStore::encrypted_file(&keyfile,KEY_PASSWORD).unwrap();
        store.save_device_keys("default",&keys).unwrap();
        store.save_ark("default",&ark).unwrap();
        store.save_account_id("default",&id).unwrap();
        let document=uuid::Uuid::new_v4().to_string();
        {
            let db=Database::open(&local.path.join("library.db")).unwrap();
            db.set_sync_identity(&id.to_hex(),keys.device_id(),0,None).unwrap();
            db.emit_change(pergamon_core::sync::event::EntityType::Document,&document,
                pergamon_core::sync::event::Op::Upsert,json!({
                    "url":"https://cli.example/local-before-auth","title":"Existing local CLI content",
                    "content_type":"article","status":"inbox","content_text":"Existing local CLI content"
                }).as_object().unwrap().clone(),vec![],1_700_000_000_000).unwrap();
        }
        success(&run(&binary,&local.path,&[
            "sync-remote","enable","--server",&url,"--relay-identity","cli-existing",
            "--register-relay-account","--key-file",keyfile.to_str().unwrap(),
        ],"recovery-fixture"));
        let mut current=DeviceKeyStore::encrypted_file(&keyfile,KEY_PASSWORD).unwrap();
        assert_eq!(current.load_account_id("default").unwrap().unwrap(),id);
        assert_eq!(current.load_ark("default").unwrap().unwrap().expose_bytes(),ark.expose_bytes());
        let loaded=current.load_device_keys("default").unwrap().unwrap();
        assert_eq!(loaded.x25519_secret(),keys.x25519_secret());
        assert_eq!(loaded.ed25519_signing(),keys.ed25519_signing());
        let mut session:binding::RemoteSession=serde_json::from_slice(
            &current.load_remote_session("default",&url).unwrap().unwrap()).unwrap();
        let old_refresh=session.refresh_token.clone();
        session.access_expires_at=0;
        current.save_remote_session("default",&url,&serde_json::to_vec(&session).unwrap()).unwrap();
        success(&run(&binary,&local.path,&["sync-remote","sync","--key-file",keyfile.to_str().unwrap()],"recovery-fixture"));
        let current=DeviceKeyStore::encrypted_file(&keyfile,KEY_PASSWORD).unwrap();
        let session:binding::RemoteSession=serde_json::from_slice(
            &current.load_remote_session("default",&url).unwrap().unwrap()).unwrap();
        assert_ne!(session.refresh_token,old_refresh,"real refresh must be durably replaced");
        assert_eq!(session.content_account_id.as_deref(),Some(id.to_hex().as_str()));
        let db=Database::open(&local.path.join("library.db")).unwrap();
        assert_eq!(db.remote_account_binding().unwrap().unwrap().state,"active");
        assert_eq!(db.sync_state().unwrap().account_id.as_deref(),Some(id.to_hex().as_str()));
        let auth=HttpAuth::new(&url).unwrap();
        let peer=DeviceKeypairs::generate().unwrap();
        let peer_session=binding::login_device(&auth,"cli-existing",PASSWORD.as_bytes(),&peer).unwrap();
        let transport=pergamon_sync::http::HttpTransport::with_credential(&url,Some(TransportCredential::Bearer {
            token:peer_session.access_token,
        })).unwrap();
        let crypto=CryptoContext::new(ark,id.to_hex(),peer.device_id().to_owned(),*peer.ed25519_signing(),0).unwrap();
        let mut directory=DeviceKeyDirectory::new();directory.insert(keys.device_id(),*keys.ed25519_verifying());
        let peer_db=Database::open_in_memory().unwrap();
        peer_db.set_sync_identity(&id.to_hex(),peer.device_id(),0,None).unwrap();
        let engine=SyncEngine::new(transport,crypto,directory);
        assert!(engine.pull(&peer_db,&MemoryBlobStore::new()).unwrap()>0);
        assert_eq!(peer_db.read_entity_fields(pergamon_core::sync::event::EntityType::Document,&document)
            .unwrap().unwrap()["title"],"Existing local CLI content");
    }).await.unwrap();
    task.abort();
    let _ = task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit PERGAMON_TEST_CLI built-binary acceptance gate"]
async fn cli_create_and_recovery_join_keep_auth_and_keys_separate() {
    let binary = binary();
    let server_home = Fixture::new();
    let (url, task) = server(&server_home.path).await;
    tokio::task::spawn_blocking(move || {
        let first = Fixture::new();
        let second = Fixture::new();
        let first_keyfile = first.path.join("keys.json");
        success(&run(
            &binary,
            &first.path,
            &[
                "sync-device",
                "bootstrap",
                "--server",
                &url,
                "--relay-identity",
                "cli-create",
                "--register-relay-account",
                "--key-file",
                first_keyfile.to_str().unwrap(),
            ],
            "correct-fixture-recovery",
        ));
        let first_keys = DeviceKeyStore::encrypted_file(&first_keyfile, KEY_PASSWORD).unwrap();
        let id = first_keys.load_account_id("default").unwrap().unwrap();
        let ark = first_keys.load_ark("default").unwrap().unwrap();
        let second_keyfile = second.path.join("keys.json");
        success(&run(
            &binary,
            &second.path,
            &[
                "sync-remote",
                "login",
                "--server",
                &url,
                "--relay-identity",
                "cli-create",
                "--join",
                "--key-file",
                second_keyfile.to_str().unwrap(),
            ],
            "wrong-fixture-recovery",
        ));
        let keyless = DeviceKeyStore::encrypted_file(&second_keyfile, KEY_PASSWORD).unwrap();
        assert!(
            keyless.load_ark("default").unwrap().is_none(),
            "PAKE login must never recover an ARK"
        );
        let failure = run(
            &binary,
            &second.path,
            &[
                "sync-device",
                "recover",
                "--server",
                &url,
                "--account-id",
                &id.to_hex(),
                "--no-pull",
                "--key-file",
                second_keyfile.to_str().unwrap(),
            ],
            "wrong-fixture-recovery",
        );
        assert!(!failure.status.success());
        assert!(
            DeviceKeyStore::encrypted_file(&second_keyfile, KEY_PASSWORD)
                .unwrap()
                .load_ark("default")
                .unwrap()
                .is_none()
        );
        success(&run(
            &binary,
            &second.path,
            &[
                "sync-device",
                "recover",
                "--server",
                &url,
                "--account-id",
                &id.to_hex(),
                "--no-pull",
                "--key-file",
                second_keyfile.to_str().unwrap(),
            ],
            "correct-fixture-recovery",
        ));
        let recovered = DeviceKeyStore::encrypted_file(&second_keyfile, KEY_PASSWORD).unwrap();
        assert_eq!(recovered.load_account_id("default").unwrap().unwrap(), id);
        assert_eq!(
            recovered
                .load_ark("default")
                .unwrap()
                .unwrap()
                .expose_bytes(),
            ark.expose_bytes()
        );
        assert_eq!(
            Database::open(&second.path.join("library.db"))
                .unwrap()
                .remote_account_binding()
                .unwrap()
                .unwrap()
                .state,
            "active"
        );
        let duplicate = run(
            &binary,
            &first.path,
            &[
                "sync-device",
                "bootstrap",
                "--server",
                &url,
                "--relay-identity",
                "cli-create",
                "--key-file",
                first_keyfile.to_str().unwrap(),
            ],
            "correct-fixture-recovery",
        );
        assert!(
            !duplicate.status.success(),
            "completed create must not invent another account"
        );
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
}
