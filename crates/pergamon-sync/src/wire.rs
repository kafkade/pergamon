// SPDX-License-Identifier: Apache-2.0

//! Client-side mirror of the server's ADR-022 wire frame types.
//!
//! These are byte-for-byte serde-compatible with the AGPL server's
//! `pergamon-sync-server::envelope` types, but defined here so the Apache-2.0
//! client never links the server crate. The server sees only these headers plus
//! the opaque `ciphertext_b64` body; the plaintext [`ChangeBody`] semantics live
//! exclusively inside that ciphertext.
//!
//! [`ChangeBody`]: pergamon_core::sync::event::ChangeBody

use serde::{Deserialize, Serialize};

/// Current wire protocol major version (ADR-022 `protocol_version`).
pub const PROTOCOL_VERSION: u32 = 1;

/// A single event envelope submitted on push: server-visible header plus opaque
/// base64 ciphertext body.
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
    /// Account key epoch that encrypted the body.
    pub key_epoch: u32,
    /// Ciphertext hashes of blobs this event depends on.
    #[serde(default)]
    pub blob_refs: Vec<String>,
    /// Opaque AEAD ciphertext body, standard-base64 encoded.
    pub ciphertext_b64: String,
}

/// A batch of event envelopes to append to one account's log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRequest {
    /// Account whose log receives the batch.
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

/// A stored event as returned on pull.
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
    #[serde(default)]
    pub blob_refs: Vec<String>,
    /// Size in bytes of the ciphertext body.
    #[serde(default)]
    pub payload_bytes: u64,
    /// Strictly monotonic per-account sequence assigned at append.
    pub server_seq: u64,
    /// Server receive time (epoch millis) — retention only, never ordering.
    #[serde(default)]
    pub server_committed_at: i64,
    /// Opaque AEAD ciphertext body, standard-base64 encoded.
    pub ciphertext_b64: String,
}

/// Response body for a pull.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResponse {
    /// Events with `server_seq > cursor`, in ascending order.
    pub events: Vec<StoredEvent>,
    /// The account's current high-water `server_seq`.
    pub high_water_seq: u64,
    /// The cursor a client should persist after applying this page.
    pub next_cursor: u64,
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
