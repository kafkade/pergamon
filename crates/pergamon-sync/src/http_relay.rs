// SPDX-License-Identifier: Apache-2.0

//! A `reqwest`-backed blocking [`RelayTransport`] (the `http` feature).
//!
//! Speaks the sync server's ADR-024 onboarding-relay endpoints against a base
//! URL, base64-encoding every opaque artifact on the wire. Kept behind the same
//! feature flag as [`crate::http::HttpTransport`] so the dependency-light engine
//! and its in-memory doubles stay usable without `reqwest`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::Serialize;

use crate::credential::TransportCredential;
use crate::error::{Result, SyncError};
use crate::http::build_client;
use crate::relay::{RelayAttestation, RelayDevice, RelayTransport, RelayWrap};

/// A blocking HTTP relay client to a pergamon sync server.
pub struct HttpRelay {
    client: Client,
    base_url: String,
}

// Serde mirrors of the server's `envelope` relay types. Kept local so this
// crate does not depend on the AGPL server crate; field names must match.

#[derive(Serialize)]
struct RecordInput {
    record_b64: String,
}
#[derive(Deserialize)]
struct DeviceEntry {
    device_id: String,
    record_b64: String,
}
#[derive(Deserialize)]
struct DevicesResponse {
    devices: Vec<DeviceEntry>,
}
#[derive(Serialize)]
struct BundleInput {
    bundle_b64: String,
}
#[derive(Deserialize)]
struct BundleAck {
    seq: u64,
}
#[derive(Deserialize)]
struct BundleEntry {
    seq: u64,
    bundle_b64: String,
}
#[derive(Deserialize)]
struct BundlesResponse {
    bundles: Vec<BundleEntry>,
}
#[derive(Serialize)]
struct AttestationInput {
    attestation_b64: String,
}
#[derive(Deserialize)]
struct AttestationAck {
    seq: u64,
}
#[derive(Deserialize)]
struct AttestationEntry {
    seq: u64,
    attestation_b64: String,
}
#[derive(Deserialize)]
struct AttestationsResponse {
    attestations: Vec<AttestationEntry>,
}
#[derive(Serialize)]
struct RecoveryInput {
    blob_b64: String,
}
#[derive(Deserialize)]
struct RecoveryResponse {
    blob_b64: String,
}

impl HttpRelay {
    /// Build a relay client for `base_url` (e.g. `https://sync.example.com`).
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the HTTP client cannot be built.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        Self::with_credential(base_url, None)
    }

    /// Build a relay client for `base_url` that authenticates every request with
    /// `credential` (e.g. HTTP Basic when the server sits behind a reverse proxy
    /// that enforces auth). Pass `None` for an unauthenticated client.
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the HTTP client cannot be built or
    /// the credential cannot be encoded as a header value. The error message
    /// never contains the credential.
    pub fn with_credential(
        base_url: impl Into<String>,
        credential: Option<TransportCredential>,
    ) -> Result<Self> {
        let client = build_client(credential)?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

/// Decode an opaque base64 relay payload received from the server.
fn decode(field: &str, value: &str) -> Result<Vec<u8>> {
    STANDARD
        .decode(value.as_bytes())
        .map_err(|e| SyncError::Protocol(format!("invalid base64 {field}: {e}")))
}

/// Map a non-success status to a transport error.
fn ensure_ok(status: StatusCode) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        Err(SyncError::Transport(format!("server returned {status}")))
    }
}

impl RelayTransport for HttpRelay {
    fn device_put(&self, account_id: &str, device_id: &str, record: &[u8]) -> Result<()> {
        let resp = self
            .client
            .put(self.url(&format!("/v1/devices/{account_id}/{device_id}")))
            .json(&RecordInput {
                record_b64: STANDARD.encode(record),
            })
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())
    }

    fn device_get(&self, account_id: &str, device_id: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .client
            .get(self.url(&format!("/v1/devices/{account_id}/{device_id}")))
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure_ok(resp.status())?;
        let entry: DeviceEntry = resp
            .json()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(Some(decode("record_b64", &entry.record_b64)?))
    }

    fn devices_list(&self, account_id: &str) -> Result<Vec<RelayDevice>> {
        let resp = self
            .client
            .get(self.url(&format!("/v1/devices/{account_id}")))
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        let body: DevicesResponse = resp
            .json()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        body.devices
            .into_iter()
            .map(|d| {
                Ok(RelayDevice {
                    device_id: d.device_id,
                    record: decode("record_b64", &d.record_b64)?,
                })
            })
            .collect()
    }

    fn wrap_put(&self, account_id: &str, device_id: &str, bundle: &[u8]) -> Result<u64> {
        let resp = self
            .client
            .post(self.url(&format!("/v1/wraps/{account_id}/{device_id}")))
            .json(&BundleInput {
                bundle_b64: STANDARD.encode(bundle),
            })
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        let ack: BundleAck = resp
            .json()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(ack.seq)
    }

    fn wraps_list(&self, account_id: &str, device_id: &str, after: u64) -> Result<Vec<RelayWrap>> {
        let resp = self
            .client
            .get(self.url(&format!("/v1/wraps/{account_id}/{device_id}")))
            .query(&[("after", after.to_string())])
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        let body: BundlesResponse = resp
            .json()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        body.bundles
            .into_iter()
            .map(|b| {
                Ok(RelayWrap {
                    seq: b.seq,
                    bundle: decode("bundle_b64", &b.bundle_b64)?,
                })
            })
            .collect()
    }

    fn attestation_append(&self, account_id: &str, attestation: &[u8]) -> Result<u64> {
        let resp = self
            .client
            .post(self.url(&format!("/v1/attestations/{account_id}")))
            .json(&AttestationInput {
                attestation_b64: STANDARD.encode(attestation),
            })
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        let ack: AttestationAck = resp
            .json()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(ack.seq)
    }

    fn attestations_list(&self, account_id: &str, after: u64) -> Result<Vec<RelayAttestation>> {
        let resp = self
            .client
            .get(self.url(&format!("/v1/attestations/{account_id}")))
            .query(&[("after", after.to_string())])
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        let body: AttestationsResponse = resp
            .json()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        body.attestations
            .into_iter()
            .map(|a| {
                Ok(RelayAttestation {
                    seq: a.seq,
                    attestation: decode("attestation_b64", &a.attestation_b64)?,
                })
            })
            .collect()
    }

    fn recovery_put(&self, account_id: &str, blob: &[u8]) -> Result<()> {
        let resp = self
            .client
            .put(self.url(&format!("/v1/recovery/{account_id}")))
            .json(&RecoveryInput {
                blob_b64: STANDARD.encode(blob),
            })
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())
    }

    fn recovery_get(&self, account_id: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .client
            .get(self.url(&format!("/v1/recovery/{account_id}")))
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure_ok(resp.status())?;
        let body: RecoveryResponse = resp
            .json()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(Some(decode("blob_b64", &body.blob_b64)?))
    }
}
