// SPDX-License-Identifier: Apache-2.0

//! V2 auth orchestration. Auth proves relay authority, never content recovery.
//! NOT EXTERNALLY SECURITY-REVIEWED: DO NOT DEPLOY.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use pergamon_crypto::DeviceKeypairs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    auth::{
        ClientLoginFlow, ClientRegistrationFlow, build_mint_pop, build_refresh_request, fresh_nonce,
    },
    error::{Result, SyncError},
};

/// An authenticated, immutable relay-local namespace binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentBinding {
    /// Relay authentication tenant; never an encrypted content header.
    pub auth_tenant_id: String,
    /// Canonical content identity; absent for unbound control authority.
    pub content_account_id: String,
    /// Durable authority revision captured at credential issuance.
    pub binding_version: i64,
    /// Binding lifecycle state; retired namespaces never authorize routing.
    pub state: String,
    /// Stable caller operation identifier for interrupted-operation recovery.
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Authenticated discovery of only the caller's namespace.
pub struct BindingStatus {
    /// Relay authentication tenant; never an encrypted content header.
    pub auth_tenant_id: String,
    /// Persistent nonsecret relay installation identifier.
    pub server_instance_id: String,
    /// The caller's one live binding, if allocated.
    pub binding: Option<ContentBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Public expiring tenant/device/namespace allocation statement.
pub struct BindingChallenge {
    /// Persistent nonsecret relay installation identifier.
    pub server_instance_id: String,
    /// Stable caller operation identifier for interrupted-operation recovery.
    pub operation_id: String,
    /// Unique handle for the expiring allocation challenge.
    pub challenge_id: String,
    /// Standard-base64 encoding of the 32-byte random challenge nonce.
    pub nonce_b64: String,
    /// Relay authentication tenant; never an encrypted content header.
    pub auth_tenant_id: String,
    /// Canonical content identity; absent for unbound control authority.
    pub content_account_id: String,
    /// Device handle cryptographically tied to the Ed25519 public key.
    pub device_id: String,
    /// Standard-base64 device verification key.
    pub ed25519_pub_b64: String,
    /// Durable authority revision captured at credential issuance.
    pub binding_version: i64,
    /// Challenge expiry in Unix epoch milliseconds.
    pub expires_at_ms: i64,
}

/// Token secrets are persisted only by the caller's unlocked secure store.
#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteSession {
    /// Explicit authority class; control sessions cannot access content.
    pub scope: SessionScope,
    /// Secret access bearer; store securely and never log it.
    pub access_token: String,
    /// Access credential expiry in Unix epoch milliseconds.
    pub access_expires_at: i64,
    /// Secret single-use refresh bearer; replace it after rotation.
    pub refresh_token: String,
    /// Refresh credential expiry in Unix epoch milliseconds.
    pub refresh_expires_at: i64,
    /// Device handle cryptographically tied to the Ed25519 public key.
    pub device_id: String,
    /// Relay authentication tenant; never an encrypted content header.
    pub auth_tenant_id: String,
    /// Canonical content identity; absent for unbound control authority.
    pub content_account_id: Option<String>,
    /// Durable authority revision captured at credential issuance.
    pub binding_version: i64,
}

/// Relay authority is independent of possession of content keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionScope {
    /// Identity and binding lifecycle operations only.
    Control,
    /// The exact authenticated canonical content namespace.
    Content,
}

impl std::fmt::Debug for RemoteSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSession")
            .field("auth_tenant_id", &self.auth_tenant_id)
            .field("content_account_id", &self.content_account_id)
            .field("device_id", &self.device_id)
            .field("tokens", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct LoginResponse {
    authenticated: bool,
    auth_tenant_id: String,
    content_account_id: Option<String>,
    binding_version: i64,
    token: Option<RemoteSession>,
}

/// Platform adapters perform I/O; all flows share these message semantics.
pub trait AuthTransport {
    /// Send an auth request without retaining plaintext passwords.
    ///
    /// # Errors
    /// Returns a transport, explicit refusal, or malformed-response error.
    fn request(&self, path: &str, body: Option<&Value>, bearer: Option<&str>) -> Result<Value>;
}

fn decoded(value: &Value, field: &str) -> Result<Vec<u8>> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| SyncError::Protocol(format!("missing auth field {field}")))?;
    Ok(STANDARD.decode(text)?)
}

/// Register without claiming a content namespace; lost replies recover via login.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn register_identity(
    transport: &impl AuthTransport,
    identity: &str,
    password: &[u8],
) -> Result<()> {
    let (flow, request) =
        ClientRegistrationFlow::start(password).map_err(|e| SyncError::Protocol(e.to_string()))?;
    let response = transport.request(
        "/v2/auth/register/start",
        Some(&json!({
            "identity_handle":identity, "registration_request_b64":STANDARD.encode(request),
        })),
        None,
    )?;
    let upload = flow
        .finish(password, &decoded(&response, "registration_response_b64")?)
        .map_err(|e| SyncError::Protocol(e.to_string()))?;
    let response = transport.request(
        "/v2/auth/register/finish",
        Some(&json!({
            "identity_handle":identity, "registration_upload_b64":STANDARD.encode(upload),
        })),
        None,
    )?;
    if response
        .get("registration_received")
        .and_then(Value::as_bool)
        != Some(true)
        || response.get("requires_login").and_then(Value::as_bool) != Some(true)
    {
        return Err(SyncError::Protocol(
            "registration did not acknowledge the login-required contract".to_owned(),
        ));
    }
    Ok(())
}

/// OPAQUE and device `PoP` establish a session, not an ARK.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn login_device(
    transport: &impl AuthTransport,
    identity: &str,
    password: &[u8],
    keys: &DeviceKeypairs,
) -> Result<RemoteSession> {
    let (flow, request) =
        ClientLoginFlow::start(password).map_err(|e| SyncError::Protocol(e.to_string()))?;
    let response = transport.request(
        "/v2/auth/login/start",
        Some(&json!({
            "identity_handle":identity,"credential_request_b64":STANDARD.encode(request),
        })),
        None,
    )?;
    let login_id = response
        .get("login_id")
        .and_then(Value::as_str)
        .ok_or_else(|| SyncError::Protocol("missing login ID".to_owned()))?;
    let finished = flow
        .finish(password, &decoded(&response, "credential_response_b64")?)
        .map_err(|e| SyncError::Protocol(e.to_string()))?;
    let proof = build_mint_pop(keys, login_id, &finished.finalization);
    let response: LoginResponse = serde_json::from_value(transport.request(
        "/v2/auth/login/finish",Some(&json!({
            "login_id":login_id,"credential_finalization_b64":STANDARD.encode(finished.finalization),
            "device_id":proof.device_id,"ed25519_pub_b64":proof.ed25519_pub_b64,
            "pop_signature_b64":proof.pop_signature_b64,
        })),None)?)?;
    let session = response
        .token
        .ok_or_else(|| SyncError::Protocol("login did not issue a device session".to_owned()))?;
    if !response.authenticated
        || session.device_id != keys.device_id()
        || session.auth_tenant_id != response.auth_tenant_id
        || session.content_account_id != response.content_account_id
        || session.binding_version != response.binding_version
        || ((session.scope == SessionScope::Content) != session.content_account_id.is_some())
    {
        return Err(SyncError::Protocol(
            "inconsistent authenticated session".to_owned(),
        ));
    }
    Ok(session)
}

/// Read only the authenticated tenant's binding.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn binding_status(
    transport: &impl AuthTransport,
    session: &RemoteSession,
) -> Result<BindingStatus> {
    let status: BindingStatus = serde_json::from_value(transport.request(
        "/v2/auth/content-binding",
        None,
        Some(&session.access_token),
    )?)?;
    if status.auth_tenant_id != session.auth_tenant_id
        || status
            .binding
            .as_ref()
            .is_some_and(|b| b.auth_tenant_id != session.auth_tenant_id)
    {
        return Err(SyncError::Protocol(
            "binding belongs to another auth tenant".to_owned(),
        ));
    }
    if status
        .binding
        .as_ref()
        .map(|b| b.content_account_id.as_str())
        != session.content_account_id.as_deref()
        || status.binding.as_ref().map_or(0, |b| b.binding_version) != session.binding_version
    {
        return Err(SyncError::Protocol(
            "binding status changed the credential authority snapshot".to_owned(),
        ));
    }
    Ok(status)
}

/// Canonical auth statement; mirrored independently by the AGPL relay.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn binding_signing_bytes(challenge: &BindingChallenge) -> Result<Vec<u8>> {
    fn lp(bytes: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let len = u32::try_from(bytes.len())
            .map_err(|_| SyncError::Protocol("binding field too long".to_owned()))?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(bytes);
        Ok(())
    }
    let nonce = STANDARD.decode(&challenge.nonce_b64)?;
    let key = STANDARD.decode(&challenge.ed25519_pub_b64)?;
    if nonce.len() != 32 || key.len() != 32 || challenge.binding_version < 0 {
        return Err(SyncError::Protocol("invalid binding statement".to_owned()));
    }
    let mut bytes = b"pergamon/v2/auth/content-binding-pop".to_vec();
    for field in [
        challenge.server_instance_id.as_bytes(),
        challenge.operation_id.as_bytes(),
        challenge.challenge_id.as_bytes(),
        &nonce,
        challenge.auth_tenant_id.as_bytes(),
        challenge.content_account_id.as_bytes(),
        challenge.device_id.as_bytes(),
    ] {
        lp(field, &mut bytes)?;
    }
    bytes.extend_from_slice(&key);
    bytes.extend_from_slice(&challenge.binding_version.to_be_bytes());
    bytes.extend_from_slice(&challenge.expires_at_ms.to_be_bytes());
    Ok(bytes)
}

/// Allocate only an empty namespace; never substitute the relay tenant ID.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn bind_empty_namespace(
    transport: &impl AuthTransport,
    session: &RemoteSession,
    keys: &DeviceKeypairs,
    content_id: &str,
    operation_id: &str,
) -> Result<ContentBinding> {
    let status = binding_status(transport, session)?;
    if let Some(binding) = status.binding.as_ref() {
        if binding.content_account_id == content_id {
            return Ok(binding.clone());
        }
        if binding.state != "legacy_reserved" {
            return Err(SyncError::Protocol(
                "relay is already bound to different content".to_owned(),
            ));
        }
    }
    let challenge: BindingChallenge = serde_json::from_value(transport.request(
        "/v2/auth/content-binding/start",
        Some(&json!({
            "operation_id":operation_id,"content_account_id":content_id,
        })),
        Some(&session.access_token),
    )?)?;
    if challenge.server_instance_id != status.server_instance_id
        || challenge.auth_tenant_id != session.auth_tenant_id
        || challenge.content_account_id != content_id
        || challenge.operation_id != operation_id
        || challenge.device_id != keys.device_id()
        || challenge.ed25519_pub_b64 != STANDARD.encode(keys.ed25519_verifying())
        || challenge.binding_version != session.binding_version
    {
        return Err(SyncError::Protocol(
            "binding challenge changed the requested identity".to_owned(),
        ));
    }
    let signature = keys.sign(&binding_signing_bytes(&challenge)?);
    let binding: ContentBinding = serde_json::from_value(transport.request(
        "/v2/auth/content-binding/finish",
        Some(&json!({
            "operation_id":operation_id,"challenge_id":challenge.challenge_id,
            "pop_signature_b64":STANDARD.encode(signature),
        })),
        Some(&session.access_token),
    )?)?;
    let expected_revision = session
        .binding_version
        .checked_add(1)
        .ok_or_else(|| SyncError::Protocol("binding revision exhausted".to_owned()))?;
    if binding.auth_tenant_id != session.auth_tenant_id
        || binding.content_account_id != content_id
        || binding.binding_version != expected_revision
    {
        return Err(SyncError::Protocol(
            "binding receipt changed the requested identity".to_owned(),
        ));
    }
    Ok(binding)
}

/// Rotate credentials without changing content authority.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn refresh_session(
    transport: &impl AuthTransport,
    keys: &DeviceKeypairs,
    session: &RemoteSession,
) -> Result<RemoteSession> {
    if session.device_id != keys.device_id() {
        return Err(SyncError::Protocol(
            "session belongs to different device keys".to_owned(),
        ));
    }
    let request = build_refresh_request(keys, &session.refresh_token, &fresh_nonce());
    let response: crate::auth::RefreshResponse = serde_json::from_value(transport.request(
        "/v2/auth/token/refresh",
        Some(&serde_json::to_value(request)?),
        None,
    )?)?;
    let mut next = session.clone();
    next.access_token = response.access_token;
    next.access_expires_at = response.access_expires_at;
    next.refresh_token = response.refresh_token;
    next.refresh_expires_at = response.refresh_expires_at;
    Ok(next)
}

/// Revoke a device within the authenticated tenant.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn revoke_device_session(
    transport: &impl AuthTransport,
    session: &RemoteSession,
    device: &str,
) -> Result<u64> {
    let response = transport.request(
        "/v2/auth/token/revoke",
        Some(&json!({"device_id":device})),
        Some(&session.access_token),
    )?;
    response
        .get("revoked")
        .and_then(Value::as_u64)
        .ok_or_else(|| SyncError::Protocol("malformed token revocation response".to_owned()))
}

/// A shared session for event/blob and onboarding transports. No password is retained.
pub struct RefreshingSession<T, P> {
    transport: T,
    keys: DeviceKeypairs,
    session: std::sync::Mutex<RemoteSession>,
    persist: P,
}

impl<T: AuthTransport, P: Fn(&RemoteSession) -> Result<()>> RefreshingSession<T, P> {
    /// Construct from an already validated, unlocked content session.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn new(
        transport: T,
        keys: DeviceKeypairs,
        session: RemoteSession,
        persist: P,
    ) -> Result<Self> {
        if session.device_id != keys.device_id()
            || session.content_account_id.is_none()
            || session.scope != SessionScope::Content
        {
            return Err(SyncError::Protocol(
                "content session does not match this device".to_owned(),
            ));
        }
        Ok(Self {
            transport,
            keys,
            session: std::sync::Mutex::new(session),
            persist,
        })
    }
}

impl<T, P> crate::credential::AccessTokenProvider for RefreshingSession<T, P>
where
    T: AuthTransport + Send + Sync,
    P: Fn(&RemoteSession) -> Result<()> + Send + Sync,
{
    fn access_token(&self) -> Result<String> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| SyncError::Protocol("session lock poisoned".to_owned()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| SyncError::Protocol("system clock predates epoch".to_owned()))?;
        let now = i64::try_from(now.as_millis())
            .map_err(|_| SyncError::Protocol("invalid system clock".to_owned()))?;
        if session.access_expires_at <= now + 30_000 {
            let next = refresh_session(&self.transport, &self.keys, &session)?;
            (self.persist)(&next)?;
            *session = next;
        }
        Ok(session.access_token.clone())
    }
}
