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
    /// The origin device's opaque Ed25519 event signature (ADR-030),
    /// standard-base64 encoded. The server stores and echoes it verbatim and
    /// never inspects it — authenticity is enforced entirely client-side.
    /// Defaults to empty for deserialization tolerance.
    #[serde(default)]
    pub sig_b64: String,
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
    /// The origin device's opaque Ed25519 event signature (ADR-030),
    /// standard-base64 encoded. Echoed verbatim, never inspected by the server.
    /// Defaults to empty for deserialization tolerance.
    #[serde(default)]
    pub sig_b64: String,
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

/// Response body for the per-tenant usage/metrics endpoint (WP-3d, #198).
///
/// Reports an account's metered ciphertext usage — **sizes and counts only**,
/// never any decoded content (design §2.5) — alongside the configured caps
/// (`0` = unlimited) and whether the account currently sits over quota. This is
/// the billing/metrics surface (`GET /v1/usage/{account_id}`); it is tenant
/// isolated in multi-tenant mode by the WP-3c `{account_id}` path-param gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageResponse {
    /// Total bytes of opaque blob ciphertext stored for the account.
    pub blob_bytes: u64,
    /// Number of distinct blobs stored for the account.
    pub blob_count: u64,
    /// Total bytes of event ciphertext (`payload_bytes`) stored for the account.
    pub event_bytes: u64,
    /// Number of events stored for the account.
    pub event_count: u64,
    /// Combined metered ciphertext bytes (`blob_bytes + event_bytes`).
    pub total_bytes: u64,
    /// Combined metered object count (`blob_count + event_count`).
    pub total_objects: u64,
    /// Configured per-account byte cap (`0` = unlimited).
    pub max_account_bytes: u64,
    /// Configured per-account object-count cap (`0` = unlimited).
    pub max_account_objects: u64,
    /// `true` when current usage already exceeds a configured cap.
    pub over_quota: bool,
}

// --- Opaque onboarding-artifact relay wire types (ADR-024, #125) -------------
//
// These carry the base64-encoded ciphertext / signed bytes of onboarding
// artifacts (device records, key-wrap bundles, attestations, recovery blob).
// The server relays them verbatim and never base64-decodes their meaning: the
// `*_b64` fields are opaque to it exactly as `ciphertext_b64` is above.

/// Request body to publish a device's opaque signed record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRecordInput {
    /// Opaque signed device-record bytes, standard-base64 encoded.
    pub record_b64: String,
}

/// A device record as returned to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRecordEntry {
    /// Opaque origin-device handle.
    pub device_id: String,
    /// Opaque signed device-record bytes, standard-base64 encoded.
    pub record_b64: String,
}

/// Response body listing an account's device roster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRecordsResponse {
    /// Every published device record for the account.
    pub devices: Vec<DeviceRecordEntry>,
}

/// Request body to relay a key-wrap bundle to a recipient device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedBundleInput {
    /// Opaque sealed bundle bytes, standard-base64 encoded.
    pub bundle_b64: String,
}

/// Response body acknowledging a relayed key-wrap bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedBundleAck {
    /// The per-recipient sequence assigned to (or already held by) the bundle.
    pub seq: u64,
    /// `true` when identical bytes already existed and were not stored again.
    pub deduplicated: bool,
}

/// A relayed key-wrap bundle as returned to its recipient.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedBundleEntry {
    /// Per-recipient monotonic sequence (the cursor domain).
    pub seq: u64,
    /// Opaque sealed bundle bytes, standard-base64 encoded.
    pub bundle_b64: String,
}

/// Response body listing pending key-wrap bundles for a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedBundlesResponse {
    /// Bundles with `seq > cursor`, ascending.
    pub bundles: Vec<WrappedBundleEntry>,
    /// The cursor to persist after applying this page (greatest `seq`, or the
    /// request cursor when empty).
    pub next_cursor: u64,
}

/// Request body to append a signed attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationInput {
    /// Opaque signed attestation bytes, standard-base64 encoded.
    pub attestation_b64: String,
}

/// Response body acknowledging an appended attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationAck {
    /// The per-account sequence assigned to (or already held by) the attestation.
    pub seq: u64,
    /// `true` when identical bytes already existed and were not stored again.
    pub deduplicated: bool,
}

/// A relayed attestation as returned to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationEntry {
    /// Per-account monotonic sequence (the cursor domain).
    pub seq: u64,
    /// Opaque signed attestation bytes, standard-base64 encoded.
    pub attestation_b64: String,
}

/// Response body listing an account's attestation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationsResponse {
    /// Attestations with `seq > cursor`, ascending.
    pub attestations: Vec<AttestationEntry>,
    /// The cursor to persist after applying this page (greatest `seq`, or the
    /// request cursor when empty).
    pub next_cursor: u64,
}

/// Request body to store an account's opaque recovery blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryBlobInput {
    /// Opaque Argon2id-wrapped recovery bytes, standard-base64 encoded.
    pub blob_b64: String,
}

/// Response body returning an account's opaque recovery blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryBlobResponse {
    /// Opaque Argon2id-wrapped recovery bytes, standard-base64 encoded.
    pub blob_b64: String,
}

/// Query parameters for a cursored relay list (bundles, attestations).
#[derive(Debug, Clone, Deserialize)]
pub struct RelayListQuery {
    /// Exclusive lower bound: return artifacts with `seq > after`.
    #[serde(default)]
    pub after: u64,
    /// Maximum number of artifacts to return in this page.
    #[serde(default)]
    pub limit: Option<u32>,
}
