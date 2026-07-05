// SPDX-License-Identifier: AGPL-3.0-only

//! Opaque onboarding-artifact relay endpoints (ADR-024, #125).
//!
//! These endpoints store and serve the artifacts that let devices onboard and
//! rotate keys — signed device records, sealed key-wrap bundles, trust /
//! revocation attestations, and the optional recovery blob. Every payload is
//! opaque ciphertext or a client signature the server cannot read; the server
//! only base64-transports and content-hash deduplicates them. This preserves the
//! blind-relay invariant: authenticity is enforced entirely client-side.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::envelope::{
    AttestationAck, AttestationEntry, AttestationInput, AttestationsResponse, DeviceRecordEntry,
    DeviceRecordInput, DeviceRecordsResponse, RecoveryBlobInput, RecoveryBlobResponse,
    RelayListQuery, WrappedBundleAck, WrappedBundleEntry, WrappedBundleInput,
    WrappedBundlesResponse,
};
use crate::error::ApiError;
use crate::state::AppState;

/// Default number of relay artifacts returned when the client omits `limit`.
const DEFAULT_LIMIT: u32 = 500;
/// Hard cap on a single relay list page.
const MAX_LIMIT: u32 = 1000;

/// Decode an opaque base64 relay payload, mapping failures to 400.
fn decode_b64(field: &str, value: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(value.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("invalid base64 {field}: {e}")))
}

/// Publish (or replace) a device's opaque signed record
/// (`PUT /v1/devices/{account_id}/{device_id}`).
///
/// # Errors
/// Returns 400 on invalid base64; 500 on a store failure.
pub async fn device_put(
    State(state): State<AppState>,
    Path((account_id, device_id)): Path<(String, String)>,
    Json(req): Json<DeviceRecordInput>,
) -> Result<StatusCode, ApiError> {
    let bytes = decode_b64("record_b64", &req.record_b64)?;
    {
        let store = state.lock_store()?;
        store.device_record_put(&account_id, &device_id, &bytes)?;
    }
    Ok(StatusCode::CREATED)
}

/// Fetch one device's opaque record
/// (`GET /v1/devices/{account_id}/{device_id}`).
///
/// # Errors
/// Returns 404 if the device has no record; 500 on a store failure.
pub async fn device_get(
    State(state): State<AppState>,
    Path((account_id, device_id)): Path<(String, String)>,
) -> Result<Json<DeviceRecordEntry>, ApiError> {
    let bytes = {
        let store = state.lock_store()?;
        store.device_record_get(&account_id, &device_id)?
    };
    match bytes {
        Some(bytes) => Ok(Json(DeviceRecordEntry {
            device_id,
            record_b64: STANDARD.encode(&bytes),
        })),
        None => Err(ApiError::not_found(format!(
            "no device record for {device_id}"
        ))),
    }
}

/// List an account's full device roster (`GET /v1/devices/{account_id}`).
///
/// # Errors
/// Returns 500 on a store failure.
pub async fn devices_list(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
) -> Result<Json<DeviceRecordsResponse>, ApiError> {
    let rows = {
        let store = state.lock_store()?;
        store.device_records_list(&account_id)?
    };
    let devices = rows
        .into_iter()
        .map(|r| DeviceRecordEntry {
            device_id: r.device_id,
            record_b64: STANDARD.encode(&r.bytes),
        })
        .collect();
    Ok(Json(DeviceRecordsResponse { devices }))
}

/// Relay a key-wrap bundle to a recipient device
/// (`POST /v1/wraps/{account_id}/{device_id}`).
///
/// # Errors
/// Returns 400 on invalid base64; 500 on a store failure.
pub async fn wrap_put(
    State(state): State<AppState>,
    Path((account_id, device_id)): Path<(String, String)>,
    Json(req): Json<WrappedBundleInput>,
) -> Result<Json<WrappedBundleAck>, ApiError> {
    let bytes = decode_b64("bundle_b64", &req.bundle_b64)?;
    let result = {
        let mut store = state.lock_store()?;
        store.wrapped_bundle_put(&account_id, &device_id, &bytes)?
    };
    Ok(Json(WrappedBundleAck {
        seq: result.seq,
        deduplicated: result.deduplicated,
    }))
}

/// List pending key-wrap bundles for a device
/// (`GET /v1/wraps/{account_id}/{device_id}?after=&limit=`).
///
/// # Errors
/// Returns 500 on a store failure.
pub async fn wraps_list(
    State(state): State<AppState>,
    Path((account_id, device_id)): Path<(String, String)>,
    Query(query): Query<RelayListQuery>,
) -> Result<Json<WrappedBundlesResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rows = {
        let store = state.lock_store()?;
        store.wrapped_bundles_list(&account_id, &device_id, query.after, limit)?
    };
    let mut next_cursor = query.after;
    let mut bundles = Vec::with_capacity(rows.len());
    for r in rows {
        next_cursor = next_cursor.max(r.seq);
        bundles.push(WrappedBundleEntry {
            seq: r.seq,
            bundle_b64: STANDARD.encode(&r.bytes),
        });
    }
    Ok(Json(WrappedBundlesResponse {
        bundles,
        next_cursor,
    }))
}

/// Append a signed attestation to an account's roster history
/// (`POST /v1/attestations/{account_id}`).
///
/// # Errors
/// Returns 400 on invalid base64; 500 on a store failure.
pub async fn attestation_append(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Json(req): Json<AttestationInput>,
) -> Result<Json<AttestationAck>, ApiError> {
    let bytes = decode_b64("attestation_b64", &req.attestation_b64)?;
    let result = {
        let mut store = state.lock_store()?;
        store.attestation_append(&account_id, &bytes)?
    };
    Ok(Json(AttestationAck {
        seq: result.seq,
        deduplicated: result.deduplicated,
    }))
}

/// List an account's attestation history
/// (`GET /v1/attestations/{account_id}?after=&limit=`).
///
/// # Errors
/// Returns 500 on a store failure.
pub async fn attestations_list(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Query(query): Query<RelayListQuery>,
) -> Result<Json<AttestationsResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rows = {
        let store = state.lock_store()?;
        store.attestations_list(&account_id, query.after, limit)?
    };
    let mut next_cursor = query.after;
    let mut attestations = Vec::with_capacity(rows.len());
    for r in rows {
        next_cursor = next_cursor.max(r.seq);
        attestations.push(AttestationEntry {
            seq: r.seq,
            attestation_b64: STANDARD.encode(&r.bytes),
        });
    }
    Ok(Json(AttestationsResponse {
        attestations,
        next_cursor,
    }))
}

/// Store (or replace) an account's opaque recovery blob
/// (`PUT /v1/recovery/{account_id}`).
///
/// # Errors
/// Returns 400 on invalid base64; 500 on a store failure.
pub async fn recovery_put(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Json(req): Json<RecoveryBlobInput>,
) -> Result<StatusCode, ApiError> {
    let bytes = decode_b64("blob_b64", &req.blob_b64)?;
    {
        let store = state.lock_store()?;
        store.recovery_blob_put(&account_id, &bytes)?;
    }
    Ok(StatusCode::CREATED)
}

/// Fetch an account's opaque recovery blob (`GET /v1/recovery/{account_id}`).
///
/// # Errors
/// Returns 404 if recovery is not enabled; 500 on a store failure.
pub async fn recovery_get(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
) -> Result<Json<RecoveryBlobResponse>, ApiError> {
    let bytes = {
        let store = state.lock_store()?;
        store.recovery_blob_get(&account_id)?
    };
    bytes.map_or_else(
        || Err(ApiError::not_found("no recovery blob for this account")),
        |bytes| {
            Ok(Json(RecoveryBlobResponse {
                blob_b64: STANDARD.encode(&bytes),
            }))
        },
    )
}
