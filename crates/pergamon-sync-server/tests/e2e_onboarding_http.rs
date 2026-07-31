// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end onboarding over real TCP for issue #128.
//!
//! The other server tests drive the axum router in-process via `oneshot`. This
//! one instead binds the router to a real `TcpListener` and drives it with the
//! client's blocking [`HttpRelay`] (the `pergamon-sync` `http` feature), proving
//! the HTTP relay client speaks the server's ADR-024 relay contract byte-for-byte
//! and that the server stays blind throughout.
//!
//! Flow: device A bootstraps a new account; device B publishes its record; A
//! approves B (sealing the ARK to it and vouching for it); B accepts and recovers
//! the Account Root Key — all over HTTP. We then assert the ARK B recovered
//! equals A's, that both devices agree on the SAS, and that the server's on-disk
//! database never contains the ARK bytes.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use pergamon_crypto::device::DeviceKeypairs;
use pergamon_crypto::hierarchy::{AccountId, AccountRootKey};
use pergamon_sync::onboarding;
use pergamon_sync::{HttpRelay, RelayTransport};

const NOW: i64 = 1_700_000_000_000;

struct TempDb {
    path: std::path::PathBuf,
}

impl TempDb {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pergamon-onboarding-http-{}.db",
            uuid::Uuid::new_v4()
        ));
        Self { path }
    }

    /// Every byte the server has persisted: the main database **plus** the WAL
    /// sidecar.
    ///
    /// Since WP-3e (#201) the store runs in WAL mode, so a just-committed row
    /// lives in `<db>-wal` until a checkpoint folds it into the main file.
    /// Reading only the main file would silently weaken the content-blindness
    /// assertion below.
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// A trusted device onboards a brand-new device over HTTP, and the new device
/// recovers the Account Root Key without the server ever seeing it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_device_onboards_over_http() {
    // --- Stand up a real server on an ephemeral TCP port. ------------------
    let tmp = TempDb::new();
    let store = pergamon_sync_server::SyncStore::open(&tmp.path).unwrap();
    let app = pergamon_sync_server::build_router(pergamon_sync_server::AppState::new(store));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://{addr}");

    // --- Shared account material (A only; B must learn the ARK over HTTP). -
    let ark = AccountRootKey::from_bytes([42u8; 32]);
    let account_id = AccountId::from_bytes([7u8; 16]);
    let epoch = 0u32;

    // The blocking relay calls run on a dedicated thread so they don't block the
    // async server task sharing this runtime.
    let ark_bytes = *ark.expose_bytes();
    let recovered = tokio::task::spawn_blocking(move || {
        let relay = HttpRelay::new(&base_url).unwrap();

        // Device A: the founding, trusted device.
        let dev_a = DeviceKeypairs::generate().unwrap();
        onboarding::bootstrap(&relay, &account_id, &dev_a, epoch, NOW).unwrap();

        // Device B: brand-new. It learns only the opaque account handle
        // out-of-band (via an invite), never the ARK.
        let dev_b = DeviceKeypairs::generate().unwrap();
        onboarding::enroll_publish(&relay, &account_id, &dev_b, NOW + 1).unwrap();

        // Both devices independently compute the SAS and it must agree.
        let sas_a = onboarding::sas_against(&relay, &account_id, &dev_a, dev_b.device_id())
            .unwrap()
            .digits();
        let sas_b = onboarding::sas_against(&relay, &account_id, &dev_b, dev_a.device_id())
            .unwrap()
            .digits();
        assert_eq!(sas_a, sas_b, "both devices must derive the same SAS");

        // Device A approves B: seals the ARK to it and vouches for it over HTTP.
        let ark = AccountRootKey::from_bytes(ark_bytes);
        onboarding::approve(
            &relay,
            &account_id,
            &dev_a,
            &ark,
            epoch,
            dev_b.device_id(),
            NOW + 2,
        )
        .unwrap();

        // Device B accepts: fetches the sealed bundle and recovers the ARK.
        let accepted = onboarding::accept(&relay, &account_id, &dev_b).unwrap();
        assert_eq!(
            accepted.approver_device_id.as_deref(),
            Some(dev_a.device_id()),
            "B should see A's trust attestation"
        );
        assert_eq!(accepted.bundle.account_id, account_id);
        assert_eq!(accepted.bundle.key_epoch, epoch);

        // Sanity: the roster now lists both devices.
        let roster = onboarding::roster(&relay, &account_id).unwrap();
        assert_eq!(roster.len(), 2, "roster should list both devices");

        // Sanity: the relay's own device_get round-trips B's published record.
        assert!(
            relay
                .device_get(&account_id.to_hex(), dev_b.device_id())
                .unwrap()
                .is_some()
        );

        accepted.bundle.ark
    })
    .await
    .unwrap();

    // The ARK B recovered over HTTP equals the one A sealed.
    assert_eq!(
        recovered.expose_bytes(),
        ark.expose_bytes(),
        "the new device must recover the exact Account Root Key"
    );

    // The server relayed only ciphertext: the ARK never hit its database.
    server.abort();
    let db_bytes = tmp.all_bytes();
    assert!(
        !contains(&db_bytes, ark.expose_bytes()),
        "the Account Root Key must never appear in the server database"
    );
}
