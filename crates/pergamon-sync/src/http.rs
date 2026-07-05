// SPDX-License-Identifier: Apache-2.0

//! A `reqwest`-backed blocking [`Transport`] (the `http` feature).
//!
//! Speaks the ADR-022 endpoints against a base URL. Kept behind a feature flag
//! so the engine and its in-memory double stay dependency-light.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use reqwest::blocking::Client;

use crate::error::{Result, SyncError};
use crate::transport::Transport;
use crate::wire::{BlobProbeRequest, BlobProbeResponse, PullResponse, PushRequest, PushResponse};

/// A blocking HTTP transport to a pergamon sync server.
pub struct HttpTransport {
    client: Client,
    base_url: String,
}

impl HttpTransport {
    /// Build a transport for `base_url` (e.g. `https://sync.example.com`).
    ///
    /// # Errors
    /// Returns a [`SyncError::Transport`] if the HTTP client cannot be built.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let client = Client::builder()
            .build()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

impl Transport for HttpTransport {
    fn push(&self, req: &PushRequest) -> Result<PushResponse> {
        let resp = self
            .client
            .post(self.url("/v1/events"))
            .json(req)
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        resp.json().map_err(|e| SyncError::Transport(e.to_string()))
    }

    fn pull(&self, account_id: &str, after: u64, limit: Option<u32>) -> Result<PullResponse> {
        let mut request = self
            .client
            .get(self.url("/v1/events"))
            .query(&[("account_id", account_id)])
            .query(&[("after", after.to_string())]);
        if let Some(limit) = limit {
            request = request.query(&[("limit", limit.to_string())]);
        }
        let resp = request
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        resp.json().map_err(|e| SyncError::Transport(e.to_string()))
    }

    fn blob_probe(&self, req: &BlobProbeRequest) -> Result<BlobProbeResponse> {
        let resp = self
            .client
            .post(self.url("/v1/blobs/probe"))
            .json(req)
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        resp.json().map_err(|e| SyncError::Transport(e.to_string()))
    }

    fn blob_put(&self, account_id: &str, ct_hash: &str, ciphertext: &[u8]) -> Result<()> {
        let resp = self
            .client
            .put(self.url(&format!("/v1/blobs/{account_id}/{ct_hash}")))
            .body(ciphertext.to_vec())
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        ensure_ok(resp.status())?;
        Ok(())
    }

    fn blob_get(&self, account_id: &str, ct_hash: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get(self.url(&format!("/v1/blobs/{account_id}/{ct_hash}")))
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Err(SyncError::MissingBlob(ct_hash.to_owned()));
        }
        ensure_ok(resp.status())?;
        let bytes = resp
            .bytes()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

/// Map a non-success status to a transport error.
fn ensure_ok(status: StatusCode) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        Err(SyncError::Transport(format!("server returned {status}")))
    }
}

/// Re-export so downstream can base64 a blob without another dependency.
#[must_use]
pub fn encode_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
