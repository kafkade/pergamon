// SPDX-License-Identifier: AGPL-3.0-only

//! Wire types for the sync protocol (ADR-022).
//!
//! These types are the **server-visible frame** only. The server reads and may
//! index the header fields below; the encrypted body is carried as an opaque,
//! base64-encoded ciphertext blob (`ciphertext_b64`) that the server never
//! decodes. Payload semantics (`entity_type`, `entity_id`, `op`, `clock`,
//! `fields`, …) live exclusively inside that ciphertext and are defined by the
//! Apache-2.0 client, never here.

use serde::{Deserialize, Serialize};

/// Current wire protocol major version (ADR-022 `protocol_version`, starts at 1).
pub const PROTOCOL_VERSION: u32 = 1;

/// A single event envelope submitted by a client on push.
///
/// Everything here is the server-visible header plus the opaque ciphertext
/// body. `server_seq` and `server_committed_at` are assigned by the server on
/// commit and therefore do not appear on input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInput {
    /// Wire protocol major version the client is speaking.
    pub protocol_version: u32,
    /// Opaque account handle this event belongs to.
    pub account_id: String,
    /// Opaque origin-device handle, so a device can suppress echoes on pull.
    pub device_id: String,
    /// Client-generated globally unique id — the idempotency key for push.
    pub change_id: String,
    /// Blinded per-entity grouping token, absent when not tied to one entity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_ref: Option<String>,
    /// Account key epoch that encrypted the body (for decryption across rotations).
    pub key_epoch: u32,
    /// Ciphertext hashes of blobs this event depends on (upload-before-commit).
    #[serde(default)]
    pub blob_refs: Vec<String>,
    /// Opaque AEAD ciphertext body, standard-base64 encoded. Never decoded by
    /// the server.
    pub ciphertext_b64: String,
}

/// A batch of event envelopes to append to one account's log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRequest {
    /// Account whose log receives the batch. Every event must match it.
    pub account_id: String,
    /// The events to append, in client order.
    pub events: Vec<EventInput>,
}

/// Per-event outcome of a push.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResult {
    /// The client-supplied idempotency key this result is for.
    pub change_id: String,
    /// The `server_seq` assigned to (or already held by) this event.
    pub server_seq: u64,
    /// `true` when the event already existed and was not appended again.
    pub deduplicated: bool,
}

/// Response body for a push.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResponse {
    /// One result per submitted event, in request order.
    pub results: Vec<PushResult>,
    /// The account's current high-water `server_seq` after the batch.
    pub high_water_seq: u64,
}

/// A stored event as returned on pull. Carries the full server-visible header,
/// the server-assigned sequence/commit time, and the opaque ciphertext body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Wire protocol major version recorded at append time.
    pub protocol_version: u32,
    /// Opaque account handle.
    pub account_id: String,
    /// Opaque origin-device handle (used by clients for echo suppression).
    pub device_id: String,
    /// Client idempotency key.
    pub change_id: String,
    /// Blinded per-entity grouping token, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_ref: Option<String>,
    /// Account key epoch that encrypted the body.
    pub key_epoch: u32,
    /// Ciphertext hashes of referenced blobs.
    pub blob_refs: Vec<String>,
    /// Size in bytes of the ciphertext body.
    pub payload_bytes: u64,
    /// Strictly monotonic per-account sequence assigned at append. The cursor
    /// domain.
    pub server_seq: u64,
    /// Server receive time (epoch millis) — retention only, never ordering.
    pub server_committed_at: i64,
    /// Opaque AEAD ciphertext body, standard-base64 encoded.
    pub ciphertext_b64: String,
}

/// Response body for a pull.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResponse {
    /// Events with `server_seq > cursor`, in ascending `server_seq` order.
    pub events: Vec<StoredEvent>,
    /// The account's current high-water `server_seq`.
    pub high_water_seq: u64,
    /// The cursor a client should persist after applying this page (the
    /// greatest `server_seq` returned, or the request cursor when empty).
    pub next_cursor: u64,
}

/// Query parameters for a pull.
#[derive(Debug, Clone, Deserialize)]
pub struct PullQuery {
    /// Account whose log to scan.
    pub account_id: String,
    /// Exclusive lower bound: return events with `server_seq > after`.
    #[serde(default)]
    pub after: u64,
    /// Maximum number of events to return in this page.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Request body for a blob dedup probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobProbeRequest {
    /// Account whose blob store to probe.
    pub account_id: String,
    /// Ciphertext hashes the client is considering uploading.
    pub ct_hashes: Vec<String>,
}

/// Response body for a blob dedup probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobProbeResponse {
    /// Hashes the server already has (do not re-upload).
    pub present: Vec<String>,
    /// Hashes the server is missing (upload these before referencing them).
    pub missing: Vec<String>,
}
