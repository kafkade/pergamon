// SPDX-License-Identifier: Apache-2.0

//! Blocking adapter for the shared v2 auth contract.

use crate::{
    account_binding::AuthTransport,
    error::{Result, SyncError},
};
use reqwest::blocking::Client;
use serde_json::Value;

/// Blocking HTTP adapter for OPAQUE and explicit content binding.
pub struct HttpAuth {
    client: Client,
    base_url: String,
}

impl HttpAuth {
    /// Verify v2 OPAQUE message support without registering an identity or namespace.
    ///
    /// # Errors
    /// Refuses unsupported/unreachable endpoints and malformed protocol responses.
    pub fn probe(&self) -> Result<()> {
        use base64::Engine as _;
        let password = pergamon_crypto::primitives::random_array::<32>()?;
        let (flow, request) = crate::auth::ClientRegistrationFlow::start(&password)
            .map_err(|_| SyncError::Protocol("could not prepare auth capability probe".into()))?;
        let response = self.request("/v2/auth/register/start", Some(&serde_json::json!({
            "identity_handle": format!("pergamon-probe-{}", uuid::Uuid::new_v4()),
            "registration_request_b64": base64::engine::general_purpose::STANDARD.encode(request),
        })), None)?;
        let encoded = response
            .get("registration_response_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                SyncError::Protocol("server does not provide the v2 OPAQUE contract".into())
            })?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        flow.finish(&password, &bytes).map_err(|_| {
            SyncError::Protocol("server returned an unsupported OPAQUE response".into())
        })?;
        Ok(())
    }

    /// Construct the adapter or session from explicit inputs.
    ///
    /// # Errors
    /// Returns an error for invalid identity, malformed wire data, or failed persistence/transport.
    pub fn new(base_url: &str) -> Result<Self> {
        let url = reqwest::Url::parse(base_url)
            .map_err(|_| SyncError::Protocol("invalid relay URL".to_owned()))?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(SyncError::Protocol(
                "relay URL must not contain credentials, query or fragment".to_owned(),
            ));
        }
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1")))
        {
            return Err(SyncError::Protocol(
                "authenticated relay requires HTTPS (except loopback tests)".to_owned(),
            ));
        }
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }
}

impl AuthTransport for HttpAuth {
    fn request(&self, path: &str, body: Option<&Value>, bearer: Option<&str>) -> Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let mut request = if let Some(body) = body {
            self.client.post(url).json(body)
        } else {
            self.client.get(url)
        };
        if let Some(bearer) = bearer {
            let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|_| SyncError::Protocol("invalid auth credential".to_owned()))?;
            value.set_sensitive(true);
            request = request.header(reqwest::header::AUTHORIZATION, value);
        }
        let response = request
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        let status = response.status();
        if status.as_u16() == 429 {
            return Err(crate::http::rate_limit_error(&response));
        }
        if status.is_server_error() {
            return Err(SyncError::Transport(format!(
                "auth server returned {status}"
            )));
        }
        if !status.is_success() {
            if status.as_u16() == 404 {
                return Err(SyncError::AuthRefused {
                    status: 404,
                    code: "AUTH_PROTOCOL_UNSUPPORTED".into(),
                });
            }
            let body: Value = crate::http::response_json(response, 65_536).map_err(|_| {
                SyncError::Protocol(format!("invalid auth error response ({status})"))
            })?;
            let code = body
                .get("code")
                .and_then(Value::as_str)
                .ok_or_else(|| SyncError::Protocol("auth refusal has no error code".to_owned()))?;
            let safe_code = match code {
                "CONTENT_NAMESPACE_UNAVAILABLE"
                | "BINDING_IMMUTABLE"
                | "OPERATION_CONFLICT"
                | "AUTH_PROTOCOL_UPGRADE_REQUIRED"
                | "UNAUTHORIZED"
                | "FORBIDDEN"
                | "BAD_REQUEST"
                | "AUTHENTICATION_FAILED"
                | "NOT_FOUND" => code,
                _ => "AUTH_REQUEST_REFUSED",
            };
            return Err(SyncError::AuthRefused {
                status: status.as_u16(),
                code: safe_code.to_owned(),
            });
        }
        crate::http::response_json(response, 65_536)
    }
}
