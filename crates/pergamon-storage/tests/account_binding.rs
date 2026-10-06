//! Nonsecret binding intent and checked, transactional local adoption.

#![allow(clippy::unwrap_used)]

use pergamon_storage::Database;

#[test]
fn pending_binding_is_idempotent_and_never_replaces_identity() {
    let db = Database::open_in_memory().unwrap();
    db.set_sync_identity("canonical", "device", 3, None)
        .unwrap();
    db.set_sync_cursor(42).unwrap();
    let first = db
        .begin_remote_binding(
            "https://relay.example/",
            "default",
            "canonical",
            "device",
            "first",
            "attach",
        )
        .unwrap();
    let second = db
        .begin_remote_binding(
            "https://relay.example",
            "default",
            "canonical",
            "device",
            "retry",
            "attach",
        )
        .unwrap();
    assert_eq!(first.operation_id, second.operation_id);
    assert_eq!(db.sync_state().unwrap().server_url, None);
    assert!(
        db.begin_remote_binding(
            "https://relay.example",
            "default",
            "different",
            "device",
            "bad",
            "attach"
        )
        .is_err()
    );
    db.identify_remote_binding("tenant", "installation")
        .unwrap();
    assert!(
        db.identify_remote_binding("different-tenant", "installation")
            .is_err()
    );
    let mut receipt = first;
    receipt.auth_tenant_id = Some("different-tenant".to_owned());
    receipt.server_instance_id = Some("installation".to_owned());
    receipt.binding_version = Some(1);
    assert!(
        db.activate_remote_binding(&receipt, 1_700_000_000_000)
            .is_err()
    );
    assert_eq!(db.sync_state().unwrap().server_url, None);
    receipt.auth_tenant_id = Some("tenant".to_owned());
    db.activate_remote_binding(&receipt, 1_700_000_000_000)
        .unwrap();
    assert_eq!(
        db.activate_remote_binding(&receipt, 1_700_000_000_000)
            .unwrap(),
        0
    );
    let local = db.sync_state().unwrap();
    assert_eq!(local.account_id.as_deref(), Some("canonical"));
    assert_eq!(local.device_id.as_deref(), Some("device"));
    assert_eq!(local.key_epoch, 3);
    assert_eq!(local.cursor_seq, 42);
    assert!(local.baseline_done);
    assert_eq!(
        db.remote_account_binding().unwrap().unwrap().state,
        "active"
    );
    assert!(
        db.begin_remote_binding(
            "https://different.example",
            "default",
            "canonical",
            "device",
            "switch",
            "attach"
        )
        .is_err()
    );
}
