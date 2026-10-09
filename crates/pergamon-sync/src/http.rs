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
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30));
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
    token_provider: Option<std::sync::Arc<dyn crate::credential::AccessTokenProvider>>,
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
            token_provider: None,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Share securely persisted rotating credentials with the onboarding relay.
    #[must_use]
    pub fn with_token_provider(
        mut self,
        provider: std::sync::Arc<dyn crate::credential::AccessTokenProvider>,
    ) -> Self {
        self.token_provider = Some(provider);
        self
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::blocking::RequestBuilder> {
        authorized_request(
            &self.client,
            method,
            self.url(path),
            self.token_provider.as_deref(),
        )
    }

    fn send(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::Response> {
        send_authorized(&self.client, request, self.token_provider.as_deref(), true)
    }
}

pub(crate) fn authorized_request(
    client: &Client,
    method: reqwest::Method,
    url: String,
    provider: Option<&dyn crate::credential::AccessTokenProvider>,
) -> Result<reqwest::blocking::RequestBuilder> {
    let mut request = client.request(method, url);
    if let Some(provider) = provider {
        let token = provider.access_token()?;
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| SyncError::Protocol("invalid session credential".to_owned()))?;
        value.set_sensitive(true);
        request = request.header(AUTHORIZATION, value);
    }
    Ok(request)
}

pub(crate) fn send_authorized(
    client: &Client,
    builder: reqwest::blocking::RequestBuilder,
    provider: Option<&dyn crate::credential::AccessTokenProvider>,
    retry: bool,
) -> Result<reqwest::blocking::Response> {
    let request = builder
        .build()
        .map_err(|_| SyncError::Protocol("invalid relay request".into()))?;
    let copy = request.try_clone();
    let rejected = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let response = client
        .execute(request)
        .map_err(|e| SyncError::Transport(e.to_string()))?;
    if response.status() != StatusCode::UNAUTHORIZED || !retry {
        return Ok(response);
    }
    let (Some(provider), Some(mut copy), Some(rejected)) = (provider, copy, rejected) else {
        return Ok(response);
    };
    provider.invalidate_access_token(&rejected)?;
    let mut value = HeaderValue::from_str(&format!("Bearer {}", provider.access_token()?))
        .map_err(|_| SyncError::Protocol("invalid session credential".into()))?;
    value.set_sensitive(true);
    copy.headers_mut().insert(AUTHORIZATION, value);
    client
        .execute(copy)
        .map_err(|e| SyncError::Transport(e.to_string()))
}

pub(crate) fn response_bytes(
    mut response: reqwest::blocking::Response,
    limit: u64,
) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    (&mut response)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SyncError::Transport("reading bounded relay response failed".into()))?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > limit) {
        return Err(SyncError::Protocol(
            "relay response exceeds the allowed size".into(),
        ));
    }
    Ok(bytes)
}

pub(crate) fn response_json<T: serde::de::DeserializeOwned>(
    response: reqwest::blocking::Response,
    limit: u64,
) -> Result<T> {
    serde_json::from_slice(&response_bytes(response, limit)?)
        .map_err(|_| SyncError::Protocol("malformed relay response".into()))
}

pub(crate) fn ensure_response(response: &reqwest::blocking::Response) -> Result<()> {
    if response.status().as_u16() == 429 {
        return Err(rate_limit_error(response));
    }
    ensure_ok(response.status())
}

pub(crate) fn rate_limit_error(response: &reqwest::blocking::Response) -> SyncError {
    let retry = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    SyncError::RateLimited {
        retry_after_seconds: retry,
    }
}

impl Transport for HttpTransport {
    fn push(&self, req: &PushRequest) -> Result<PushResponse> {
        let resp = self.send(self.request(reqwest::Method::POST, "/v1/events")?.json(req))?;
        ensure_response(&resp)?;
        response_json(resp, 67_108_864)
    }

    fn pull(&self, account_id: &str, after: u64, limit: Option<u32>) -> Result<PullResponse> {
        let mut request = self
            .request(reqwest::Method::GET, "/v1/events")?
            .query(&[("account_id", account_id)])
            .query(&[("after", after.to_string())]);
        if let Some(limit) = limit {
            request = request.query(&[("limit", limit.to_string())]);
        }
        let resp = self.send(request)?;
        ensure_response(&resp)?;
        response_json(resp, 67_108_864)
    }

    fn blob_probe(&self, req: &BlobProbeRequest) -> Result<BlobProbeResponse> {
        let resp = self.send(
            self.request(reqwest::Method::POST, "/v1/blobs/probe")?
                .json(req),
        )?;
        ensure_response(&resp)?;
        response_json(resp, 67_108_864)
    }

    fn blob_put(&self, account_id: &str, ct_hash: &str, ciphertext: &[u8]) -> Result<()> {
        let resp = self.send(
            self.request(
                reqwest::Method::PUT,
                &format!("/v1/blobs/{account_id}/{ct_hash}"),
            )?
            .body(ciphertext.to_vec()),
        )?;
        ensure_response(&resp)?;
        Ok(())
    }

    fn blob_get(&self, account_id: &str, ct_hash: &str) -> Result<Vec<u8>> {
        let resp = self.send(self.request(
            reqwest::Method::GET,
            &format!("/v1/blobs/{account_id}/{ct_hash}"),
        )?)?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Err(SyncError::MissingBlob(ct_hash.to_owned()));
        }
        ensure_response(&resp)?;
        response_bytes(resp, 67_108_864)
    }
}

/// Map a non-success status to a transport error.
fn ensure_ok(status: StatusCode) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else if status.is_client_error() && status.as_u16() != 429 {
        Err(SyncError::AuthRefused {
            status: status.as_u16(),
            code: "CONTENT_REQUEST_REFUSED".to_owned(),
        })
    } else {
        Err(SyncError::Transport(format!("server returned {status}")))
    }
}

/// Re-export so downstream can base64 a blob without another dependency.
#[must_use]
pub fn encode_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
