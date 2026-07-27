// SPDX-License-Identifier: AGPL-3.0-only

//! Event-log push and pull endpoints.

use axum::Json;
use axum::extract::{Query, State};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::envelope::{
    PROTOCOL_VERSION, PullQuery, PullResponse, PushRequest, PushResponse, PushResult, StoredEvent,
};
use crate::error::ApiError;
use crate::state::AppState;
use crate::store::EventRecord;

/// Default number of events returned by a pull when the client omits `limit`.
const DEFAULT_LIMIT: u32 = 500;
/// Hard cap on a single pull page.
const MAX_LIMIT: u32 = 1000;

/// Push a batch of event envelopes (`POST /v1/events`).
///
/// Validates each event against the batch account and the supported protocol
/// version, decodes its opaque base64 ciphertext body, then appends the batch
/// atomically. Dedupe on `change_id` and upload-before-commit are enforced by
/// the store, making the call idempotent under retry.
///
/// # Errors
/// Returns 400 for a mismatched account, unsupported protocol version, or
/// invalid base64; 409 if an event references an un-uploaded blob; 500 on a
/// store failure.
pub async fn push(
    State(state): State<AppState>,
    Json(req): Json<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
    let mut records = Vec::with_capacity(req.events.len());
    for ev in &req.events {
        if ev.account_id != req.account_id {
            return Err(ApiError::bad_request(format!(
                "event account_id {:?} does not match batch account_id {:?}",
                ev.account_id, req.account_id
            )));
        }
        if ev.protocol_version != PROTOCOL_VERSION {
            return Err(ApiError::bad_request(format!(
                "unsupported protocol_version {}; server speaks {PROTOCOL_VERSION}",
                ev.protocol_version
            )));
        }
        let ciphertext = STANDARD.decode(ev.ciphertext_b64.as_bytes()).map_err(|e| {
            ApiError::bad_request(format!(
                "invalid base64 ciphertext for change_id {}: {e}",
                ev.change_id
            ))
        })?;
        // The signature is opaque to the server: decode it only to store raw
        // bytes and echo them back verbatim (ADR-030). Never inspected.
        let signature = STANDARD.decode(ev.sig_b64.as_bytes()).map_err(|e| {
            ApiError::bad_request(format!(
                "invalid base64 signature for change_id {}: {e}",
                ev.change_id
            ))
        })?;
        records.push(EventRecord {
            protocol_version: ev.protocol_version,
            account_id: ev.account_id.clone(),
            device_id: ev.device_id.clone(),
            change_id: ev.change_id.clone(),
            entity_ref: ev.entity_ref.clone(),
            key_epoch: ev.key_epoch,
            blob_refs: ev.blob_refs.clone(),
            ciphertext,
            signature,
        });
    }

    let outcome = {
        let mut store = state.lock_store()?;
        store.push_events(&req.account_id, &records)?
    };

    let results = outcome
        .results
        .into_iter()
        .map(|r| PushResult {
            change_id: r.change_id,
            server_seq: r.server_seq,
            deduplicated: r.deduplicated,
        })
        .collect();

    Ok(Json(PushResponse {
        results,
        high_water_seq: outcome.high_water_seq,
    }))
}

/// Pull a page of events (`GET /v1/events?account_id=&after=&limit=`).
///
/// Returns events with `server_seq > after` in ascending order, base64-encoding
/// each opaque ciphertext body for transport. The response also carries the
/// account high-water mark and the cursor to persist after applying the page.
///
/// # Errors
/// Returns 500 on a store failure.
pub async fn pull(
    State(state): State<AppState>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let (records, high_water_seq) = {
        let store = state.lock_store()?;
        let records = store.pull_events(&query.account_id, query.after, limit)?;
        let high_water = store.high_water(&query.account_id)?;
        drop(store);
        (records, high_water)
    };

    let mut next_cursor = query.after;
    let mut events = Vec::with_capacity(records.len());
    for r in records {
        next_cursor = next_cursor.max(r.server_seq);
        events.push(StoredEvent {
            protocol_version: r.protocol_version,
            account_id: r.account_id,
            device_id: r.device_id,
            change_id: r.change_id,
            entity_ref: r.entity_ref,
            key_epoch: r.key_epoch,
            blob_refs: r.blob_refs,
            payload_bytes: r.payload_bytes,
            server_seq: r.server_seq,
            server_committed_at: r.server_committed_at,
            ciphertext_b64: STANDARD.encode(&r.ciphertext),
            sig_b64: STANDARD.encode(&r.signature),
        });
    }

    Ok(Json(PullResponse {
        events,
        high_water_seq,
        next_cursor,
    }))
}
