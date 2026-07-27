// SPDX-License-Identifier: AGPL-3.0-only

//! Two-database convergence acceptance test for issue #126, driven over the
//! **real** `pergamon-sync-server` axum router (not the in-memory transport
//! double).
//!
//! This is the issue's headline acceptance criterion: two independent local
//! `SQLite` databases, each with its own [`SyncEngine`], push and pull encrypted
//! events through the actual server router and converge to byte-identical state
//! — including per-field LWW, conflict-copy for concurrent prose, observed-remove
//! membership edges, and deletes. The server only ever sees opaque ciphertext.
//!
//! Per ADR-008 the AGPL server crate may take Apache-2.0 client crates as
//! dev-dependencies; the server never links them at runtime.
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pergamon_core::sync::event::{EntityType, Op};
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use pergamon_sync::error::Result as SyncResult;
use pergamon_sync::wire::{
    BlobProbeRequest, BlobProbeResponse, PullResponse, PushRequest, PushResponse,
};
use pergamon_sync::{
    CryptoContext, DeviceKeyDirectory, MemoryBlobStore, SyncEngine, SyncError, Transport,
};
use tower::ServiceExt;

use serde_json::{Value, json};

const ACCOUNT_SEED: [u8; 16] = [9u8; 16];
const ARK_SEED: [u8; 32] = [11u8; 32];

fn account_hex() -> String {
    AccountId::from_bytes(ACCOUNT_SEED).to_hex()
}

/// Deterministic per-device Ed25519 signing seed for tests (ADR-030).
fn signing_seed(device: &str) -> [u8; 32] {
    let mut seed = [0u8; 32];
    let bytes = device.as_bytes();
    let n = bytes.len().min(32);
    seed[..n].copy_from_slice(&bytes[..n]);
    seed
}

/// A directory mapping the two test devices to their Ed25519 public keys, so the
/// engine can verify each pulled event's signature against its author.
fn directory() -> DeviceKeyDirectory {
    let mut dir = DeviceKeyDirectory::new();
    for device in ["device-a", "device-b"] {
        dir.insert(
            device,
            pergamon_crypto::primitives::ed25519_public(&signing_seed(device)),
        );
    }
    dir
}

fn crypto(device: &str) -> CryptoContext {
    CryptoContext::new(
        AccountRootKey::from_bytes(ARK_SEED),
        account_hex(),
        device.to_owned(),
        signing_seed(device),
        0,
    )
    .unwrap()
}

/// A [`Transport`] that drives the real server router in-process via
/// `oneshot`, blocking a current-thread runtime per call. The acceptance test
/// runs on a plain (non-async) thread, so `block_on` is safe here.
struct RouterTransport {
    app: Router,
    rt: Arc<tokio::runtime::Runtime>,
}

impl RouterTransport {
    fn new(app: Router) -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        Self {
            app,
            rt: Arc::new(rt),
        }
    }

    fn roundtrip(&self, req: Request<Body>) -> (StatusCode, Vec<u8>) {
        self.rt.block_on(async {
            let resp = self.app.clone().oneshot(req).await.unwrap();
            let status = resp.status();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, body.to_vec())
        })
    }
}

fn transport_err(msg: impl Into<String>) -> SyncError {
    SyncError::Transport(msg.into())
}

impl Transport for RouterTransport {
    fn push(&self, req: &PushRequest) -> SyncResult<PushResponse> {
        let body = serde_json::to_vec(req).map_err(|e| transport_err(e.to_string()))?;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/events")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .map_err(|e| transport_err(e.to_string()))?;
        let (status, bytes) = self.roundtrip(request);
        if !status.is_success() {
            return Err(transport_err(format!("push -> {status}")));
        }
        serde_json::from_slice(&bytes).map_err(|e| transport_err(e.to_string()))
    }

    fn pull(&self, account_id: &str, after: u64, limit: Option<u32>) -> SyncResult<PullResponse> {
        use std::fmt::Write as _;
        let mut uri = format!("/v1/events?account_id={account_id}&after={after}");
        if let Some(limit) = limit {
            let _ = write!(uri, "&limit={limit}");
        }
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .map_err(|e| transport_err(e.to_string()))?;
        let (status, bytes) = self.roundtrip(request);
        if !status.is_success() {
            return Err(transport_err(format!("pull -> {status}")));
        }
        serde_json::from_slice(&bytes).map_err(|e| transport_err(e.to_string()))
    }

    fn blob_probe(&self, req: &BlobProbeRequest) -> SyncResult<BlobProbeResponse> {
        let body = serde_json::to_vec(req).map_err(|e| transport_err(e.to_string()))?;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/blobs/probe")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .map_err(|e| transport_err(e.to_string()))?;
        let (status, bytes) = self.roundtrip(request);
        if !status.is_success() {
            return Err(transport_err(format!("blob_probe -> {status}")));
        }
        serde_json::from_slice(&bytes).map_err(|e| transport_err(e.to_string()))
    }

    fn blob_put(&self, account_id: &str, ct_hash: &str, ciphertext: &[u8]) -> SyncResult<()> {
        let request = Request::builder()
            .method("PUT")
            .uri(format!("/v1/blobs/{account_id}/{ct_hash}"))
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(ciphertext.to_vec()))
            .map_err(|e| transport_err(e.to_string()))?;
        let (status, _) = self.roundtrip(request);
        if !status.is_success() {
            return Err(transport_err(format!("blob_put -> {status}")));
        }
        Ok(())
    }

    fn blob_get(&self, account_id: &str, ct_hash: &str) -> SyncResult<Vec<u8>> {
        let request = Request::builder()
            .method("GET")
            .uri(format!("/v1/blobs/{account_id}/{ct_hash}"))
            .body(Body::empty())
            .map_err(|e| transport_err(e.to_string()))?;
        let (status, bytes) = self.roundtrip(request);
        if status == StatusCode::NOT_FOUND {
            return Err(SyncError::MissingBlob(ct_hash.to_owned()));
        }
        if !status.is_success() {
            return Err(transport_err(format!("blob_get -> {status}")));
        }
        Ok(bytes)
    }
}

fn server_router() -> Router {
    let store = pergamon_sync_server::SyncStore::open_in_memory().unwrap();
    pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store))
}

fn synced_db(device: &str) -> Database {
    let db = Database::open_in_memory().unwrap();
    db.set_sync_identity(&account_hex(), device, 0, Some("router://test"))
        .unwrap();
    db
}

fn fields(pairs: &[(&str, Value)]) -> FieldMap {
    let mut m = FieldMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    m
}

fn read(db: &Database, et: EntityType, id: &str) -> Option<FieldMap> {
    db.read_entity_fields(et, id).unwrap()
}

/// Drive both engines to a fixpoint (push then pull, repeated) so every event
/// each device produced is observed and applied by the other.
fn settle(
    eng_a: &SyncEngine<RouterTransport>,
    db_a: &Database,
    blob_a: &MemoryBlobStore,
    eng_b: &SyncEngine<RouterTransport>,
    db_b: &Database,
    blob_b: &MemoryBlobStore,
) {
    for _ in 0..3 {
        eng_a.sync(db_a, blob_a).unwrap();
        eng_b.sync(db_b, blob_b).unwrap();
        eng_a.sync(db_a, blob_a).unwrap();
    }
}

#[test]
fn two_databases_converge_over_the_real_server() {
    let app = server_router();
    let eng_a = SyncEngine::new(
        RouterTransport::new(app.clone()),
        crypto("device-a"),
        directory(),
    );
    let eng_b = SyncEngine::new(
        RouterTransport::new(app.clone()),
        crypto("device-b"),
        directory(),
    );
    let db_a = synced_db("device-a");
    let db_b = synced_db("device-b");
    let (blob_a, blob_b) = (MemoryBlobStore::new(), MemoryBlobStore::new());

    // A creates a document; both sync so B has it.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields(&[
            ("title", json!("Original title")),
            ("content_type", json!("article")),
            ("status", json!("inbox")),
            ("content_text", json!("first body")),
        ]),
        vec![],
        1_000,
    )
    .unwrap();
    settle(&eng_a, &db_a, &blob_a, &eng_b, &db_b, &blob_b);

    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b, "document must replicate A -> B over the real server");
    assert_eq!(b.get("title").unwrap(), &json!("Original title"));

    // Concurrent, non-overlapping field edits (per-field LWW): A edits status,
    // B edits title.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("status", json!("archived"))]),
        vec![],
        2_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("title", json!("Renamed"))]),
        vec![],
        2_050,
    )
    .unwrap();
    settle(&eng_a, &db_a, &blob_a, &eng_b, &db_b, &blob_b);

    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b, "per-field edits must converge");
    assert_eq!(a.get("status").unwrap(), &json!("archived"));
    assert_eq!(a.get("title").unwrap(), &json!("Renamed"));

    // Concurrent prose edits on the same field -> conflict-copy on both sides,
    // both converge on the same winner.
    db_a.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("content_text", json!("A rewrote the body"))]),
        vec![],
        3_000,
    )
    .unwrap();
    db_b.emit_change(
        EntityType::Document,
        "doc-1",
        Op::FieldPatch,
        fields(&[("content_text", json!("B rewrote the body"))]),
        vec![],
        3_010,
    )
    .unwrap();
    settle(&eng_a, &db_a, &blob_a, &eng_b, &db_b, &blob_b);

    let a = read(&db_a, EntityType::Document, "doc-1").unwrap();
    let b = read(&db_b, EntityType::Document, "doc-1").unwrap();
    assert_eq!(a, b, "concurrent prose edits must converge to one winner");
    let conflicts_a = db_a.list_conflicts(false).unwrap();
    let conflicts_b = db_b.list_conflicts(false).unwrap();
    assert_eq!(conflicts_a.len(), 1, "device A preserves the losing prose");
    assert_eq!(conflicts_b.len(), 1, "device B preserves the losing prose");
    assert_eq!(conflicts_a[0].loser_value, conflicts_b[0].loser_value);

    // Tag + membership edge (observed-remove set): A adds a tag, links the doc.
    db_a.emit_change(
        EntityType::Tag,
        "tag-1",
        Op::Upsert,
        fields(&[("name", json!("rust"))]),
        vec![],
        4_000,
    )
    .unwrap();
    db_a.emit_change(
        EntityType::TagEdge,
        "doc-1:tag-1",
        Op::Upsert,
        fields(&[("present", json!(true))]),
        vec![],
        4_050,
    )
    .unwrap();
    settle(&eng_a, &db_a, &blob_a, &eng_b, &db_b, &blob_b);

    assert!(
        read(&db_b, EntityType::Tag, "tag-1").is_some(),
        "tag must replicate to B"
    );
    assert!(
        db_b.set_edge(EntityType::TagEdge, "doc-1:tag-1")
            .unwrap()
            .is_present(),
        "membership edge must replicate to B"
    );

    // Delete wins: B deletes the document; A observes the tombstone.
    db_b.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Delete,
        FieldMap::new(),
        vec![],
        5_000,
    )
    .unwrap();
    settle(&eng_a, &db_a, &blob_a, &eng_b, &db_b, &blob_b);

    assert!(
        read(&db_a, EntityType::Document, "doc-1").is_none(),
        "delete must propagate B -> A"
    );
    assert!(
        read(&db_b, EntityType::Document, "doc-1").is_none(),
        "document stays deleted on B"
    );

    // Server blindness: the raw events on the wire must not leak plaintext.
    let transport = RouterTransport::new(app);
    let pulled = transport.pull(&account_hex(), 0, None).unwrap();
    assert!(!pulled.events.is_empty(), "server retained the event log");
    for ev in &pulled.events {
        let ct = STANDARD.decode(&ev.ciphertext_b64).unwrap();
        assert!(
            !contains(&ct, b"Original title"),
            "ciphertext must not contain plaintext title"
        );
        assert!(
            !contains(&ct, b"A rewrote the body"),
            "ciphertext must not contain plaintext body"
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}
