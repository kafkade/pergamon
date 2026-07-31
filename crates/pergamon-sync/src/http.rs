// SPDX-License-Identifier: Apache-2.0

//! A `reqwest`-backed blocking [`Transport`] (the `http` feature).
//!
//! Speaks the ADR-022 endpoints against a base URL. Kept behind a feature flag
//! so the engine and its in-memory double stay dependency-light.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use crate::credential::TransportCredential;
use crate::error::{Result, SyncError};
use crate::transport::Transport;
use crate::wire::{BlobProbeRequest, BlobProbeResponse, PullResponse, PushRequest, PushResponse};

/// Build a blocking [`Client`] that attaches `credential` (if any) as a
/// sensitive `Authorization` default header on every request.
///
/// The header is marked sensitive (`HeaderValue::set_sensitive(true)`) so
/// `reqwest` and its logging never record the secret. A credential that cannot
/// be encoded into a valid header value surfaces as a [`SyncError::Transport`]
/// whose message never includes the credential text.
pub(crate) fn build_client(credential: Option<TransportCredential>) -> Result<Client> {
    let mut builder = Client::builder();
    if let Some(credential) = credential {
        let mut value = HeaderValue::from_str(&credential.authorization_header_value())
            .map_err(|_| SyncError::Transport("invalid authorization credential".to_owned()))?;
        value.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, value);
        builder = builder.default_headers(headers);
    }
    builder
        .build()
        .map_err(|e| SyncError::Transport(e.to_string()))
}

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
        Self::with_credential(base_url, None)
    }

    /// Build a transport for `base_url` that authenticates every request with
    /// `credential` (e.g. HTTP Basic when the server sits behind a reverse proxy
    /// that enforces auth). Pass `None` for an unauthenticated transport.
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
