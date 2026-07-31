// SPDX-License-Identifier: Apache-2.0

//! Known-answer vectors and cross-module round-trips for the pergamon E2EE
//! scheme (ADR-024).
//!
//! The hard-coded hex values pin the concrete output of each deterministic
//! derivation so an accidental change to a domain-separation label, encoding, or
//! primitive is caught immediately — including across platforms (CLI, iOS, WASM).
//! If any of these vectors change, the wire format has changed and every client
//! must be updated in lockstep.

#![allow(clippy::unwrap_used)]

use pergamon_crypto::device::DeviceKeypairs;
use pergamon_crypto::hierarchy::{ACCOUNT_ID_LEN, AccountId, AccountRootKey};
use pergamon_crypto::{
    EventHeader, RewrapRecipient, attest_revoke, decrypt_blob, decrypt_event, enable_recovery,
    encrypt_blob, encrypt_event, entity_ref, open_rewrapped, recover, rotate_and_rewrap,
};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Fixed ARK used across the deterministic vectors.
fn fixed_ark() -> AccountRootKey {
    AccountRootKey::from_bytes([1u8; 32])
}

#[test]
fn kat_account_content_key_epoch0() {
    let ack = fixed_ark().content_key(0).unwrap();
    assert_eq!(
        hex(ack.expose_bytes()),
        "e7e9e589329f6d124ee7e1faa132031d6e991608405f3cad0a9b56f9cba3f58c"
    );
}

#[test]
fn kat_account_stream_key() {
    let ask = fixed_ark().account_stream_key().unwrap();
    assert_eq!(
        hex(&*ask),
        "43b036bc20f5d864fc2275ebd9855d7a70184bad27f2f85f2a2184b62193804f"
    );
}

#[test]
fn kat_event_key() {
    let ack = fixed_ark().content_key(0).unwrap();
    let key = ack.event_key("chg-1").unwrap();
    assert_eq!(
        hex(&*key),
        "98d26d16f73f711a8a187a5c71a3f7fbb2ac016ac113d1391184805f9a629beb"
    );
}

#[test]
fn kat_entity_ref_blinding() {
    let ask = fixed_ark().account_stream_key().unwrap();
    let blinded = entity_ref(&ask, "document", "doc-42").unwrap();
    assert_eq!(
        blinded,
        "ea78526d991d03461a8c61e3beb17d5020f606868a88484a3335b6782122ef1d"
    );
}

#[test]
fn kat_convergent_blob_is_deterministic() {
    let ack = fixed_ark().content_key(0).unwrap();
    let blob = encrypt_blob(&ack, b"hello convergent world").unwrap();
    assert_eq!(
        blob.ct_hash,
        "07541a6788611a0659f73a0fb18535e2356a472e7d8efb250a4230f6b359de47"
    );
    assert_eq!(
        hex(&blob.plaintext_hash),
        "43adbfaf7ed6f5a1c6c7538c938048f255bea60d31177bdacdd6a73a85da8673"
    );
    assert_eq!(
        hex(&blob.ciphertext),
        "9b3deca19b1242d253fcb49d9e9b145c2e161b8e600b40d49969b357040cc485e0fca7b880b3"
    );

    // Re-encrypting the same plaintext under the same epoch is byte-identical
    // (convergent), which is what makes ADR-022 ct_hash dedup work under E2EE.
    let again = encrypt_blob(&ack, b"hello convergent world").unwrap();
    assert_eq!(again.ct_hash, blob.ct_hash);
    assert_eq!(again.ciphertext, blob.ciphertext);

    // A different epoch re-encrypts to a different ciphertext (no cross-epoch
    // linkability).
    let ack1 = fixed_ark().content_key(1).unwrap();
    let other = encrypt_blob(&ack1, b"hello convergent world").unwrap();
    assert_ne!(other.ct_hash, blob.ct_hash);
}

#[test]
fn blob_roundtrip_and_tamper_rejected() {
    let ack = fixed_ark().content_key(0).unwrap();
    let blob = encrypt_blob(&ack, b"payload bytes").unwrap();
    let pt = decrypt_blob(&ack, &blob.plaintext_hash, &blob.ciphertext).unwrap();
    assert_eq!(pt, b"payload bytes");

    let mut bad = blob.ciphertext.clone();
    bad[0] ^= 0xff;
    assert!(decrypt_blob(&ack, &blob.plaintext_hash, &bad).is_err());
}

fn header() -> EventHeader {
    EventHeader {
        protocol_version: 1,
        account_id: "0123456789abcdef0123456789abcdef".to_owned(),
        device_id: "device-kat".to_owned(),
        change_id: "chg-1".to_owned(),
        key_epoch: 0,
        entity_ref: Some("blinded-kat".to_owned()),
        blob_refs: vec!["hashA".to_owned(), "hashB".to_owned()],
    }
}

#[test]
fn event_roundtrip_and_aad_binding() {
    let ack = fixed_ark().content_key(0).unwrap();
    let h = header();
    let ct = encrypt_event(&ack, &h, b"secret event body").unwrap();
    assert_ne!(ct, b"secret event body");

    let pt = decrypt_event(&ack, &h, &ct).unwrap();
    assert_eq!(pt, b"secret event body");

    // Mutating any AAD-bound header field breaks authentication.
    let mut tampered = header();
    tampered.blob_refs = vec!["hashA".to_owned()];
    assert!(decrypt_event(&ack, &tampered, &ct).is_err());
}

#[test]
fn recovery_roundtrip_and_wrong_passphrase() {
    let ark = fixed_ark();
    let id = AccountId::from_bytes([5u8; ACCOUNT_ID_LEN]);
    let blob = enable_recovery(&ark, &id, b"a strong passphrase").unwrap();
    let recovered = recover(&blob, &id, b"a strong passphrase").unwrap();
    assert_eq!(recovered.expose_bytes(), ark.expose_bytes());
    assert!(recover(&blob, &id, b"nope").is_err());
}

#[test]
fn rotation_excludes_revoked_device_and_attests() {
    let ark = fixed_ark();
    let id = AccountId::from_bytes([6u8; ACCOUNT_ID_LEN]);
    let keeper = DeviceKeypairs::generate().unwrap();
    let revoked = DeviceKeypairs::generate().unwrap();

    // Rotate to epoch 1, re-wrapping only to the retained device.
    let recipients = [RewrapRecipient {
        device_id: keeper.device_id(),
        x25519_pub: keeper.x25519_public(),
    }];
    let (ack1, wraps) = rotate_and_rewrap(&ark, &id, 1, &recipients).unwrap();
    assert_eq!(wraps.len(), 1);

    let unwrapped = open_rewrapped(
        keeper.x25519_secret(),
        keeper.device_id(),
        &id,
        1,
        &wraps[0].sealed,
    )
    .unwrap();
    assert_eq!(unwrapped.expose_bytes(), ack1.expose_bytes());

    // The keeper signs a revocation attestation over the revoked device record.
    let revoked_record = revoked.sign_record(1_700_000_000_000).record;
    let att = attest_revoke(&keeper, &revoked_record, 1, 1_700_000_000_100);
    att.verify().unwrap();
    assert_eq!(att.attestation.key_epoch, 1);
}
