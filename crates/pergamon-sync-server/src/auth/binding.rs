// SPDX-License-Identifier: AGPL-3.0-only

//! Explicit tenant/content binding. NOT EXTERNALLY SECURITY-REVIEWED: DO NOT DEPLOY.

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::{RngCore as _, rngs::OsRng};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    AuthState, routes,
    store::ValidatedToken,
    token::{self, TokenKind},
};
use crate::{error::ApiError, state::AppState};

/// The one live namespace owned by a relay authentication tenant.
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

/// Authenticated discovery of the caller's binding, never arbitrary ID lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingStatus {
    /// Relay authentication tenant; never an encrypted content header.
    pub auth_tenant_id: String,
    /// Persistent nonsecret relay installation identifier.
    pub server_instance_id: String,
    /// The caller's one live binding, if allocated.
    pub binding: Option<ContentBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Request an authenticated allocation challenge.
pub struct BindingStart {
    /// Stable caller operation identifier for interrupted-operation recovery.
    pub operation_id: String,
    /// Canonical content identity; absent for unbound control authority.
    pub content_account_id: String,
}

/// Public, domain-separated allocation statement. This does not prove ARK possession.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Signed completion of an authenticated allocation challenge.
pub struct BindingFinish {
    /// Stable caller operation identifier for interrupted-operation recovery.
    pub operation_id: String,
    /// Unique handle for the expiring allocation challenge.
    pub challenge_id: String,
    /// Standard-base64 Ed25519 proof over the binding challenge.
    pub pop_signature_b64: String,
}

#[derive(Clone)]
struct BindingState {
    auth: AuthState,
    content: AppState,
}

pub(crate) fn now_ms() -> i64 {
    i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .unwrap_or(i64::MAX)
}

pub(crate) fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if version == 1 {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE content_bindings (
            content_account_id TEXT PRIMARY KEY NOT NULL,
            auth_tenant_id TEXT NOT NULL REFERENCES account_map(account_id),
            state TEXT NOT NULL CHECK(state IN ('legacy_reserved','active','retired')),
            binding_version INTEGER NOT NULL CHECK(binding_version >= 0),
            operation_id TEXT,
            bound_by_device_id TEXT,
            bound_by_ed25519_pub BLOB
         );
         CREATE UNIQUE INDEX one_live_binding ON content_bindings(auth_tenant_id)
             WHERE state != 'retired';
         INSERT INTO content_bindings
             (content_account_id,auth_tenant_id,state,binding_version)
             SELECT account_id,account_id,'legacy_reserved',0 FROM account_map;
         ALTER TABLE tokens ADD COLUMN content_account_id TEXT;
         ALTER TABLE tokens ADD COLUMN binding_version INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE tokens ADD COLUMN scope TEXT NOT NULL DEFAULT 'content'
             CHECK(scope IN ('control','content'));
         UPDATE tokens SET content_account_id = account_id;
         CREATE TABLE binding_challenges (
             auth_tenant_id TEXT NOT NULL REFERENCES account_map(account_id),
             operation_id TEXT NOT NULL,
             challenge_json TEXT NOT NULL,
             expires_at INTEGER NOT NULL,
             PRIMARY KEY(auth_tenant_id,operation_id)
         );
         CREATE TABLE auth_metadata (name TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL);
         ALTER TABLE account_map RENAME COLUMN account_id TO auth_tenant_id;
         ALTER TABLE tokens RENAME COLUMN account_id TO auth_tenant_id;
         PRAGMA user_version = 1;",
    )?;
    tx.execute(
        "INSERT INTO auth_metadata VALUES ('server_instance_id', ?1)",
        params![Uuid::new_v4().simple().to_string()],
    )?;
    tx.commit()
}

pub(crate) fn binding_for(
    conn: &Connection,
    tenant: &str,
) -> Result<Option<ContentBinding>, rusqlite::Error> {
    conn.query_row(
        "SELECT content_account_id,binding_version,state,operation_id
         FROM content_bindings WHERE auth_tenant_id=?1 AND state!='retired'",
        params![tenant],
        |r| {
            Ok(ContentBinding {
                auth_tenant_id: tenant.to_owned(),
                content_account_id: r.get(0)?,
                binding_version: r.get(1)?,
                state: r.get(2)?,
                operation_id: r.get(3)?,
            })
        },
    )
    .optional()
}

fn server_id(conn: &Connection) -> Result<String, ApiError> {
    conn.query_row(
        "SELECT value FROM auth_metadata WHERE name='server_instance_id'",
        [],
        |r| r.get(0),
    )
    .map_err(|e| {
        tracing::error!(error=%e, "binding metadata unavailable");
        ApiError::internal("binding metadata unavailable")
    })
}

pub(crate) fn bearer(headers: &HeaderMap) -> Result<String, ApiError> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("invalid bearer token"))?;
    let (scheme, value) = raw
        .split_once(' ')
        .filter(|(scheme, value)| scheme.eq_ignore_ascii_case("Bearer") && !value.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("invalid bearer token"))?;
    let _ = scheme;
    Ok(value.trim().to_owned())
}

fn principal(auth: &AuthState, credential: &str) -> Result<ValidatedToken, ApiError> {
    auth.lock_store()?
        .load_valid_token(credential, TokenKind::Access)?
        .ok_or_else(|| ApiError::unauthorized("invalid bearer token"))
}

pub(crate) fn conflict(code: &'static str, message: &'static str) -> ApiError {
    let mut error = ApiError::conflict(message);
    error.code = code;
    error
}

/// The auth statement uses the existing Ed25519 and unambiguous framing.
///
/// # Errors
/// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
pub fn signing_bytes(challenge: &BindingChallenge) -> Result<Vec<u8>, ApiError> {
    fn lp(bytes: &[u8], output: &mut Vec<u8>) -> Result<(), ApiError> {
        let len = u32::try_from(bytes.len())
            .map_err(|_| ApiError::bad_request("binding field too long"))?;
        output.extend_from_slice(&len.to_be_bytes());
        output.extend_from_slice(bytes);
        Ok(())
    }
    let nonce = STANDARD
        .decode(&challenge.nonce_b64)
        .map_err(|_| ApiError::bad_request("invalid nonce"))?;
    let key = STANDARD
        .decode(&challenge.ed25519_pub_b64)
        .map_err(|_| ApiError::bad_request("invalid key"))?;
    if nonce.len() != 32 || key.len() != 32 || challenge.binding_version < 0 {
        return Err(ApiError::bad_request("invalid binding statement"));
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

/// Mount the versioned identity and binding endpoints.
pub fn router(auth: AuthState, content: AppState) -> Router {
    let identity = Router::new()
        .route("/v2/auth/register/start", post(routes::register_start))
        .route("/v2/auth/register/finish", post(routes::register_finish_v2))
        .route("/v2/auth/login/start", post(routes::login_start))
        .route("/v2/auth/login/finish", post(routes::login_finish_v2))
        .route("/v2/auth/token/refresh", post(routes::token_refresh))
        .route("/v2/auth/token/revoke", post(routes::token_revoke))
        .with_state(auth.clone());
    identity.merge(
        Router::new()
            .route("/v2/auth/content-binding", get(status))
            .route("/v2/auth/content-binding/start", post(start))
            .route("/v2/auth/content-binding/finish", post(finish))
            .with_state(BindingState { auth, content }),
    )
}

async fn status(
    State(state): State<BindingState>,
    headers: HeaderMap,
) -> Result<Json<BindingStatus>, ApiError> {
    let credential = bearer(&headers)?;
    let who = principal(&state.auth, &credential)?;
    let store = state.auth.lock_store()?;
    Ok(Json(BindingStatus {
        server_instance_id: server_id(store.connection())?,
        binding: store.binding(&who.auth_tenant_id)?,
        auth_tenant_id: who.auth_tenant_id,
    }))
}

async fn start(
    State(state): State<BindingState>,
    headers: HeaderMap,
    Json(request): Json<BindingStart>,
) -> Result<Json<BindingChallenge>, ApiError> {
    let credential = bearer(&headers)?;
    let who = principal(&state.auth, &credential)?;
    if request.operation_id.is_empty()
        || request.operation_id.len() > 128
        || request.content_account_id.len() != 32
        || !request
            .content_account_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ApiError::bad_request(
            "invalid binding operation or canonical content ID",
        ));
    }
    let store = state.auth.lock_store()?;
    let conn = store.connection();
    if let Some(binding) = store.binding(&who.auth_tenant_id)?
        && binding.operation_id.as_deref() == Some(request.operation_id.as_str())
        && binding.content_account_id != request.content_account_id
    {
        return Err(conflict(
            "OPERATION_CONFLICT",
            "binding operation has different parameters",
        ));
    }
    conn.execute(
        "DELETE FROM binding_challenges WHERE expires_at<=?1",
        params![now_ms()],
    )
    .map_err(super::store::AuthStoreError::from)?;
    let old: Option<String> = conn.query_row(
        "SELECT challenge_json FROM binding_challenges WHERE auth_tenant_id=?1 AND operation_id=?2",
        params![who.auth_tenant_id,request.operation_id], |r| r.get(0))
        .optional().map_err(super::store::AuthStoreError::from)?;
    if let Some(old) = old {
        let challenge: BindingChallenge = serde_json::from_str(&old)
            .map_err(|_| ApiError::internal("corrupt binding challenge"))?;
        if challenge.content_account_id != request.content_account_id
            || challenge.device_id != who.device_id
            || challenge.ed25519_pub_b64 != STANDARD.encode(who.ed25519_pub)
        {
            return Err(conflict(
                "OPERATION_CONFLICT",
                "binding operation has different parameters",
            ));
        }
        return Ok(Json(challenge));
    }
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM binding_challenges", [], |r| r.get(0))
        .map_err(super::store::AuthStoreError::from)?;
    if count >= 10_000 {
        return Err(ApiError::unavailable("too many binding attempts"));
    }
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let challenge = BindingChallenge {
        server_instance_id: server_id(conn)?,
        operation_id: request.operation_id,
        challenge_id: Uuid::new_v4().simple().to_string(),
        nonce_b64: STANDARD.encode(nonce),
        auth_tenant_id: who.auth_tenant_id.clone(),
        content_account_id: request.content_account_id,
        device_id: who.device_id,
        ed25519_pub_b64: STANDARD.encode(who.ed25519_pub),
        binding_version: who.binding_version,
        expires_at_ms: now_ms() + 120_000,
    };
    conn.execute(
        "INSERT INTO binding_challenges VALUES (?1,?2,?3,?4)",
        params![
            challenge.auth_tenant_id,
            challenge.operation_id,
            serde_json::to_string(&challenge)
                .map_err(|_| ApiError::internal("binding encoding failed"))?,
            challenge.expires_at_ms
        ],
    )
    .map_err(super::store::AuthStoreError::from)?;
    drop(store);
    Ok(Json(challenge))
}

async fn finish(
    State(state): State<BindingState>,
    headers: HeaderMap,
    Json(request): Json<BindingFinish>,
) -> Result<Json<ContentBinding>, ApiError> {
    let credential = bearer(&headers)?;
    let who = principal(&state.auth, &credential)?;
    let auth = state.auth.clone();
    let content = state.content.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _lease = content.binding_gate.write()
            .map_err(|_| ApiError::internal("binding gate poisoned"))?;
        let slot = content.tenants.acquire(&who.auth_tenant_id)?;
        let result = content.store.with_namespace_barrier(|occupied| {
            let mut store = auth.lock_store()?;
            let who = store.load_valid_token(&credential, TokenKind::Access)?
                .ok_or_else(|| ApiError::unauthorized("invalid bearer token"))?;
            let tx = store.connection_mut().transaction()
                .map_err(super::store::AuthStoreError::from)?;
            let current = binding_for(&tx, &who.auth_tenant_id).map_err(super::store::AuthStoreError::from)?;
            if let Some(binding) = &current
                && binding.operation_id.as_deref()==Some(&request.operation_id)
            {
                return Ok(binding.clone());
            }
            let raw: Option<String> = tx.query_row(
                "SELECT challenge_json FROM binding_challenges WHERE auth_tenant_id=?1 AND operation_id=?2",
                params![who.auth_tenant_id,request.operation_id], |r| r.get(0))
                .optional().map_err(super::store::AuthStoreError::from)?;
            let challenge: BindingChallenge = serde_json::from_str(&raw
                .ok_or_else(|| ApiError::unauthorized("invalid binding challenge"))?)
                .map_err(|_| ApiError::internal("corrupt binding challenge"))?;
            let sig: [u8;64] = STANDARD.decode(&request.pop_signature_b64)
                .map_err(|_| ApiError::bad_request("invalid binding signature"))?
                .try_into().map_err(|_| ApiError::bad_request("invalid binding signature length"))?;
            if challenge.challenge_id!=request.challenge_id || challenge.expires_at_ms<=now_ms()
                || challenge.auth_tenant_id!=who.auth_tenant_id || challenge.device_id!=who.device_id
                || challenge.ed25519_pub_b64!=STANDARD.encode(who.ed25519_pub)
                || challenge.binding_version!=who.binding_version
                || !token::verify_ed25519(&who.ed25519_pub, &signing_bytes(&challenge)?, &sig)
            {
                return Err(ApiError::unauthorized("invalid binding proof"));
            }
            if let Some(binding) = &current {
                if binding.content_account_id==challenge.content_account_id { return Ok(binding.clone()); }
                if binding.state!="legacy_reserved" || occupied(&binding.content_account_id)? {
                    return Err(conflict("BINDING_IMMUTABLE", "established content binding cannot change"));
                }
            }
            let reserved: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM content_bindings WHERE content_account_id=?1)",
                params![challenge.content_account_id], |r| r.get(0))
                .map_err(super::store::AuthStoreError::from)?;
            if reserved || occupied(&challenge.content_account_id)? {
                tracing::warn!(target:"pergamon::auth::audit", auth_tenant_id=%who.auth_tenant_id,
                    "content namespace allocation refused");
                return Err(conflict("CONTENT_NAMESPACE_UNAVAILABLE", "content namespace unavailable"));
            }
            tx.execute("UPDATE content_bindings SET state='retired' WHERE auth_tenant_id=?1",
                params![who.auth_tenant_id]).map_err(super::store::AuthStoreError::from)?;
            let binding = ContentBinding {
                auth_tenant_id: who.auth_tenant_id, content_account_id: challenge.content_account_id,
                binding_version: who.binding_version.checked_add(1)
                    .ok_or_else(||ApiError::internal("binding revision exhausted"))?, state:"active".to_owned(),
                operation_id:Some(request.operation_id.clone()),
            };
            tx.execute(
                "INSERT INTO content_bindings
                 (content_account_id,auth_tenant_id,state,binding_version,operation_id,
                  bound_by_device_id,bound_by_ed25519_pub) VALUES (?1,?2,'active',?3,?4,?5,?6)",
                params![binding.content_account_id,binding.auth_tenant_id,binding.binding_version,
                    request.operation_id,who.device_id,who.ed25519_pub.as_slice()],
            ).map_err(super::store::AuthStoreError::from)?;
            tx.execute("UPDATE tokens SET revoked_at=?2 WHERE auth_tenant_id=?1 AND revoked_at IS NULL",
                params![binding.auth_tenant_id,now_ms()]).map_err(super::store::AuthStoreError::from)?;
            tx.execute("DELETE FROM binding_challenges WHERE auth_tenant_id=?1",
                params![binding.auth_tenant_id]).map_err(super::store::AuthStoreError::from)?;
            tx.commit().map_err(super::store::AuthStoreError::from)?;
            drop(store);
            Ok(binding)
        });
        drop(slot);
        result
    }).await.map_err(|e| {
        tracing::error!(error=%e, "binding task failed");
        ApiError::internal("binding task failed")
    })??;
    Ok(Json(result))
}
