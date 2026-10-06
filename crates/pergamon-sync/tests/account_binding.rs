//! Shared credential persistence and namespace-consistency guards.

#![cfg(feature = "auth")]
#![allow(clippy::unwrap_used)]

use pergamon_crypto::DeviceKeypairs;
use pergamon_sync::{
    SyncError,
    account_binding::{
        AuthTransport, RefreshingSession, RemoteSession, SessionScope, binding_status,
    },
    credential::AccessTokenProvider,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct RefreshTransport {
    calls: Arc<AtomicUsize>,
}
impl AuthTransport for RefreshTransport {
    fn request(
        &self,
        path: &str,
        body: Option<&Value>,
        _bearer: Option<&str>,
    ) -> pergamon_sync::error::Result<Value> {
        assert_eq!(path, "/v2/auth/token/refresh");
        assert!(body.unwrap().get("pop_signature_b64").is_some());
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(
            json!({"access_token":"new-access","access_expires_at":9_000_000_000_000_i64,
            "refresh_token":"new-refresh","refresh_expires_at":9_000_000_000_000_i64}),
        )
    }
}

fn session(keys: &DeviceKeypairs) -> RemoteSession {
    RemoteSession {
        scope: SessionScope::Content,
        access_token: "old-access".to_owned(),
        access_expires_at: 0,
        refresh_token: "row.c2VjcmV0".to_owned(),
        refresh_expires_at: 9_000_000_000_000,
        device_id: keys.device_id().to_owned(),
        auth_tenant_id: "tenant".to_owned(),
        content_account_id: Some("canonical".to_owned()),
        binding_version: 1,
    }
}

#[test]
fn refreshed_authority_is_exposed_only_after_durable_persistence() {
    let keys = DeviceKeypairs::generate().unwrap();
    let initial = session(&keys);
    let calls = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let stored = writes.clone();
    let provider = RefreshingSession::new(
        RefreshTransport {
            calls: calls.clone(),
        },
        keys,
        initial,
        move |next: &RemoteSession| {
            assert_eq!(next.refresh_token, "new-refresh");
            stored.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(provider.access_token().unwrap(), "new-access");
    assert_eq!(provider.access_token().unwrap(), "new-access");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    let keys = DeviceKeypairs::generate().unwrap();
    let initial = session(&keys);
    let failed = RefreshingSession::new(
        RefreshTransport { calls },
        keys,
        initial,
        |_: &RemoteSession| {
            Err(SyncError::Protocol(
                "injected persistence failure".to_owned(),
            ))
        },
    )
    .unwrap();
    assert!(failed.access_token().is_err());
    let debug = format!("{:?}", session(&DeviceKeypairs::generate().unwrap()));
    assert!(!debug.contains("old-access"));
    assert!(!debug.contains("c2VjcmV0"));
}

struct ChangedBinding;
impl AuthTransport for ChangedBinding {
    fn request(
        &self,
        _path: &str,
        _body: Option<&Value>,
        _bearer: Option<&str>,
    ) -> pergamon_sync::error::Result<Value> {
        Ok(
            json!({"auth_tenant_id":"tenant","server_instance_id":"installation",
            "binding":{"auth_tenant_id":"tenant","content_account_id":"different",
                "binding_version":1,"state":"active","operation_id":"op"}}),
        )
    }
}

#[test]
fn returned_binding_must_match_the_token_snapshot() {
    let keys = DeviceKeypairs::generate().unwrap();
    assert!(binding_status(&ChangedBinding, &session(&keys)).is_err());
    let mut control = session(&keys);
    control.scope = SessionScope::Control;
    control.content_account_id = None;
    assert!(
        RefreshingSession::new(ChangedBinding, keys, control, |_: &RemoteSession| Ok(())).is_err()
    );
}
