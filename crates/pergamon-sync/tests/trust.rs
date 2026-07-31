// SPDX-License-Identifier: Apache-2.0

//! Trust-hardening tests (ADR-030): per-device event signatures and AAD identity
//! binding.
//!
//! These prove that a hostile or rolled-back relay cannot forge, re-attribute,
//! or re-route an event without the pulling device detecting it:
//!
//! * a validly signed event from a known device applies;
//! * a tampered ciphertext or signature is rejected as a non-retryable
//!   [`SyncError::BadEventSignature`] and never applied;
//! * re-attributing an event to a *different known* device fails signature
//!   verification (the signature was made by the real author over its own
//!   `device_id`), so it is likewise rejected;
//! * an event from a device absent from the roster is a retryable
//!   [`SyncError::UnknownSigner`] (the roster may just be stale);
//! * tampering the AAD-bound `device_id` or `entity_ref` breaks AEAD decryption
//!   independently of the signature check.
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use pergamon_core::sync::event::{EntityType, Op};
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_storage::Database;
use pergamon_storage::sync::FieldMap;
use pergamon_sync::error::Result as SyncResult;
use pergamon_sync::wire::{
    BlobProbeRequest, BlobProbeResponse, PullResponse, PushRequest, PushResponse, StoredEvent,
};
use pergamon_sync::{
    CryptoContext, DeviceKeyDirectory, MemoryBlobStore, MemoryTransport, SyncEngine, SyncError,
    Transport,
};
use serde_json::json;

const ACCOUNT_SEED: [u8; 16] = [5u8; 16];
const ARK_SEED: [u8; 32] = [8u8; 32];

fn account_hex() -> String {
    AccountId::from_bytes(ACCOUNT_SEED).to_hex()
}

/// Deterministic per-device Ed25519 signing seed for tests.
fn signing_seed(device: &str) -> [u8; 32] {
    let mut seed = [0u8; 32];
    let bytes = device.as_bytes();
    let n = bytes.len().min(32);
    seed[..n].copy_from_slice(&bytes[..n]);
    seed
}

/// A directory mapping each named device to its Ed25519 public key.
fn directory(devices: &[&str]) -> DeviceKeyDirectory {
    let mut dir = DeviceKeyDirectory::new();
    for device in devices {
        dir.insert(
            *device,
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

fn synced_db(device: &str) -> Database {
    let db = Database::open_in_memory().unwrap();
    db.set_sync_identity(&account_hex(), device, 0, Some("mem://test"))
        .unwrap();
    db
}

fn seed_doc(db: &Database) {
    let mut fields = FieldMap::new();
    fields.insert("title".to_owned(), json!("Hello"));
    fields.insert("content_type".to_owned(), json!("article"));
    fields.insert("status".to_owned(), json!("inbox"));
    db.emit_change(
        EntityType::Document,
        "doc-1",
        Op::Upsert,
        fields,
        vec![],
        1_000,
    )
    .unwrap();
}

/// A shared "server" holding exactly one validly signed event authored by
/// `device-a`.
fn transport_with_one_event() -> MemoryTransport {
    let transport = MemoryTransport::new();
    let db_a = synced_db("device-a");
    seed_doc(&db_a);
    let eng_a = SyncEngine::new(
        transport.clone(),
        crypto("device-a"),
        directory(&["device-a", "device-b"]),
    );
    let pushed = eng_a.push(&db_a, &MemoryBlobStore::new()).unwrap();
    assert_eq!(pushed, 1);
    transport
}

/// Read one validly signed [`StoredEvent`] back off the server for crypto-level
/// tampering tests.
fn one_stored_event() -> StoredEvent {
    let transport = transport_with_one_event();
    let page = transport.pull(&account_hex(), 0, None).unwrap();
    page.events.into_iter().next().unwrap()
}

fn doc_applied(db: &Database) -> bool {
    db.read_entity_fields(EntityType::Document, "doc-1")
        .unwrap()
        .is_some()
}

/// A relay wrapper that mutates every event on pull, modelling a hostile or
/// rolled-back server that tries to tamper with the log.
struct HostileTransport {
    inner: MemoryTransport,
    tamper: Box<dyn Fn(&mut StoredEvent)>,
}

impl HostileTransport {
    fn new(inner: MemoryTransport, tamper: impl Fn(&mut StoredEvent) + 'static) -> Self {
        Self {
            inner,
            tamper: Box::new(tamper),
        }
    }
}

impl Transport for HostileTransport {
    fn push(&self, req: &PushRequest) -> SyncResult<PushResponse> {
        self.inner.push(req)
    }

    fn pull(&self, account_id: &str, after: u64, limit: Option<u32>) -> SyncResult<PullResponse> {
        let mut resp = self.inner.pull(account_id, after, limit)?;
        for ev in &mut resp.events {
            (self.tamper)(ev);
        }
        Ok(resp)
    }

    fn blob_probe(&self, req: &BlobProbeRequest) -> SyncResult<BlobProbeResponse> {
        self.inner.blob_probe(req)
    }

    fn blob_put(&self, account_id: &str, ct_hash: &str, ciphertext: &[u8]) -> SyncResult<()> {
        self.inner.blob_put(account_id, ct_hash, ciphertext)
    }

    fn blob_get(&self, account_id: &str, ct_hash: &str) -> SyncResult<Vec<u8>> {
        self.inner.blob_get(account_id, ct_hash)
    }
}

/// Pull `device-b` against a hostile server that applies `tamper`, returning the
/// resulting error (the tests here all expect rejection).
fn pull_tampered(
    known_devices: &[&str],
    tamper: impl Fn(&mut StoredEvent) + 'static,
) -> (SyncError, bool) {
    let hostile = HostileTransport::new(transport_with_one_event(), tamper);
    let db_b = synced_db("device-b");
    let eng_b = SyncEngine::new(hostile, crypto("device-b"), directory(known_devices));
    let err = eng_b.pull(&db_b, &MemoryBlobStore::new()).unwrap_err();
    (err, doc_applied(&db_b))
}

#[test]
fn valid_signed_event_from_known_device_applies() {
    let transport = transport_with_one_event();
    let db_b = synced_db("device-b");
    let eng_b = SyncEngine::new(
        transport,
        crypto("device-b"),
        directory(&["device-a", "device-b"]),
    );
    let applied = eng_b.pull(&db_b, &MemoryBlobStore::new()).unwrap();
    assert_eq!(applied, 1);
    assert!(doc_applied(&db_b));
}

#[test]
fn tampered_ciphertext_is_rejected_as_bad_signature() {
    let (err, applied) = pull_tampered(&["device-a", "device-b"], |ev| {
        let mut ct = STANDARD.decode(&ev.ciphertext_b64).unwrap();
        ct[0] ^= 0xff;
        ev.ciphertext_b64 = STANDARD.encode(ct);
    });
    assert!(
        matches!(err, SyncError::BadEventSignature { .. }),
        "expected BadEventSignature, got {err:?}"
    );
    assert!(!err.is_retryable(), "a forged event must not be retryable");
    assert!(!applied, "a rejected event must not be applied");
}

#[test]
fn tampered_signature_is_rejected_as_bad_signature() {
    let (err, applied) = pull_tampered(&["device-a", "device-b"], |ev| {
        let mut sig = STANDARD.decode(&ev.sig_b64).unwrap();
        sig[0] ^= 0xff;
        ev.sig_b64 = STANDARD.encode(sig);
    });
    assert!(matches!(err, SyncError::BadEventSignature { .. }));
    assert!(!applied);
}

#[test]
fn reattributing_to_a_different_known_device_is_rejected() {
    // The relay swaps the author to another *known* device. The signature was
    // made by the real author over its own device_id, so it cannot verify under
    // the impostor's key: re-attribution is caught.
    let (err, applied) = pull_tampered(&["device-a", "device-b", "device-c"], |ev| {
        ev.device_id = "device-c".to_owned();
    });
    assert!(matches!(err, SyncError::BadEventSignature { .. }));
    assert!(!applied);
}

#[test]
fn event_from_unknown_device_is_retryable_unknown_signer() {
    let (err, applied) = pull_tampered(&["device-a", "device-b"], |ev| {
        ev.device_id = "device-z".to_owned();
    });
    assert!(
        matches!(err, SyncError::UnknownSigner { .. }),
        "expected UnknownSigner, got {err:?}"
    );
    assert!(
        err.is_retryable(),
        "an unknown signer is transient (stale roster) and should be retryable"
    );
    assert!(!applied);
}

#[test]
fn empty_signature_from_known_device_is_rejected() {
    let (err, applied) = pull_tampered(&["device-a", "device-b"], |ev| {
        ev.sig_b64 = String::new();
    });
    assert!(matches!(err, SyncError::BadEventSignature { .. }));
    assert!(!applied);
}

#[test]
fn decrypt_rejects_tampered_entity_ref() {
    // AAD binding is independent of the signature check: even if a signature
    // somehow verified, a re-routed entity_ref breaks AEAD decryption.
    let ctx = crypto("device-a");
    let mut ev = one_stored_event();
    assert!(ctx.decrypt_change(&ev).is_ok(), "baseline must decrypt");
    ev.entity_ref = Some("forged-routing-token".to_owned());
    assert!(
        ctx.decrypt_change(&ev).is_err(),
        "a tampered entity_ref must fail decryption"
    );
}

#[test]
fn decrypt_rejects_dropped_entity_ref() {
    let ctx = crypto("device-a");
    let mut ev = one_stored_event();
    ev.entity_ref = None;
    assert!(
        ctx.decrypt_change(&ev).is_err(),
        "dropping the entity_ref must fail decryption (None != Some)"
    );
}

#[test]
fn decrypt_rejects_tampered_device_id() {
    let ctx = crypto("device-a");
    let mut ev = one_stored_event();
    ev.device_id = "device-x".to_owned();
    assert!(
        ctx.decrypt_change(&ev).is_err(),
        "a re-attributed device_id must fail decryption"
    );
}
