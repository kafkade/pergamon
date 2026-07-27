// SPDX-License-Identifier: AGPL-3.0-only

//! Content-addressed blob endpoints: dedup probe, upload, and download.

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::auth::AuthAccount;
use crate::auth::authorize_account;
use crate::envelope::{BlobProbeRequest, BlobProbeResponse};
use crate::error::ApiError;
use crate::state::AppState;

/// Probe which of a set of ciphertext hashes the server already holds
/// (`POST /v1/blobs/probe`).
///
/// The client uploads only the `missing` ones before referencing them from an
/// event, so the log never gains a dangling reference.
///
/// **WP-3c tenant isolation (#197):** `account_id` is in the request **body**,
/// not the path. An unauthorized probe would be a cross-tenant blob-existence
/// oracle, so when the middleware injected an [`AuthAccount`] (multi-tenant mode)
/// this handler asserts the token's `account_id` equals `req.account_id` before
/// touching the store. Blind mode has no extension and is unchanged.
///
/// # Errors
/// Returns 401/403 in multi-tenant mode when the caller is unauthenticated or
/// targets another tenant; 500 on a store failure.
pub async fn probe(
    State(state): State<AppState>,
    maybe_auth: Option<Extension<AuthAccount>>,
    Json(req): Json<BlobProbeRequest>,
) -> Result<Json<BlobProbeResponse>, ApiError> {
    if let Some(Extension(auth)) = maybe_auth {
        authorize_account(&auth, &req.account_id, "POST", "/v1/blobs/probe")?;
    }
    let (present, missing) = {
        let store = state.lock_store()?;
        store.blob_probe(&req.account_id, &req.ct_hashes)?
    };
    Ok(Json(BlobProbeResponse { present, missing }))
}

/// Upload an opaque blob (`PUT /v1/blobs/{account_id}/{ct_hash}`).
///
/// The body is stored verbatim as ciphertext. The server verifies the supplied
/// `ct_hash` matches the SHA-256 of the bytes and is idempotent: re-uploading
/// an existing blob is a no-op.
///
/// # Errors
/// Returns 400 if the hash does not match the bytes; 500 on a store failure.
pub async fn put(
    State(state): State<AppState>,
    Path((account_id, ct_hash)): Path<(String, String)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    {
        let store = state.lock_store()?;
        store.blob_put(&account_id, &ct_hash, &body)?;
    }
    Ok(StatusCode::CREATED)
}

/// Download an opaque blob (`GET /v1/blobs/{account_id}/{ct_hash}`).
///
/// Returns the raw ciphertext bytes as `application/octet-stream`.
///
/// # Errors
/// Returns 404 if the account has no blob with that hash; 500 on a store
/// failure.
pub async fn get(
    State(state): State<AppState>,
    Path((account_id, ct_hash)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let bytes = {
        let store = state.lock_store()?;
        store.blob_get(&account_id, &ct_hash)?
    };
    bytes.map_or_else(
        || {
            Err(ApiError::not_found(format!(
                "no blob {ct_hash} for this account"
            )))
        },
        |bytes| Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response()),
    )
}
