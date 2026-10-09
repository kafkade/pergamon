// SPDX-License-Identifier: AGPL-3.0-only

//! Mandatory local-operator and synchronizer-token protection for sync setup.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use pergamon_crypto::primitives;
use url::Url;

use crate::state::AppState;

const COOKIE_NAME: &str = "pergamon_sync_operator";
const SESSION_LIFETIME: Duration = Duration::from_mins(30);

struct Session {
    csrf: String,
    expires: Instant,
}

/// Expiring form sessions for the process's single configured Basic-auth owner.
pub struct OperatorSessions {
    origin: Url,
    sessions: Mutex<HashMap<String, Session>>,
}

/// Request-local form token and optional cookie issued on a protected GET.
#[derive(Clone)]
pub struct OperatorContext {
    /// Synchronizer token for HTML forms; never a URL parameter.
    pub csrf: String,
    cookie: Option<HeaderValue>,
}

/// Loopback cleartext is an explicit development exception, not a LAN policy.
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    }
}

impl OperatorSessions {
    /// Pin the external origin; forwarded headers never determine trust.
    pub fn new(origin: &str, allow_insecure_loopback: bool) -> Result<Self> {
        let url = Url::parse(origin)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || !(url.scheme() == "https"
                || (url.scheme() == "http" && allow_insecure_loopback && is_loopback(&url)))
        {
            bail!(
                "web origin must be HTTPS without credentials/path/query; loopback HTTP requires the development opt-in"
            );
        }
        Ok(Self {
            origin: url,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    fn destination_matches(&self, headers: &HeaderMap) -> bool {
        let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
            return false;
        };
        if host.contains(['/', '@', '?', '#']) {
            return false;
        }
        Url::parse(&format!("{}://{host}", self.origin.scheme()))
            .is_ok_and(|url| url.origin() == self.origin.origin())
    }

    fn source_matches(&self, headers: &HeaderMap) -> bool {
        if let Some(origin) = headers.get(header::ORIGIN) {
            return origin
                .to_str()
                .ok()
                .is_some_and(|origin| origin == self.origin.origin().ascii_serialization());
        }
        if let Some(referer) = headers.get(header::REFERER) {
            return referer
                .to_str()
                .ok()
                .and_then(|s| Url::parse(s).ok())
                .is_some_and(|url| url.origin() == self.origin.origin());
        }
        true
    }

    fn context(&self, headers: &HeaderMap, create: bool) -> Result<OperatorContext, StatusCode> {
        let cookie = headers
            .get(header::COOKIE)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| {
                s.split(';').find_map(|part| {
                    let (name, value) = part.trim().split_once('=')?;
                    (name == COOKIE_NAME).then_some(value)
                })
            });
        let now = Instant::now();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        sessions.retain(|_, session| session.expires > now);
        if let Some(cookie) = cookie {
            let hash = URL_SAFE_NO_PAD.encode(primitives::blake3_hash(cookie.as_bytes()));
            if let Some(session) = sessions.get_mut(&hash) {
                session.expires = now + SESSION_LIFETIME;
                return Ok(OperatorContext {
                    csrf: session.csrf.clone(),
                    cookie: None,
                });
            }
        }
        if !create {
            return Err(StatusCode::FORBIDDEN);
        }
        if sessions.len() >= 32 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let cookie = URL_SAFE_NO_PAD.encode(
            primitives::random_array::<32>().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        let csrf = URL_SAFE_NO_PAD.encode(
            primitives::random_array::<32>().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        sessions.insert(
            URL_SAFE_NO_PAD.encode(primitives::blake3_hash(cookie.as_bytes())),
            Session {
                csrf: csrf.clone(),
                expires: now + SESSION_LIFETIME,
            },
        );
        drop(sessions);
        let secure = if self.origin.scheme() == "https" {
            "; Secure"
        } else {
            ""
        };
        let value = HeaderValue::from_str(&format!(
            "{COOKIE_NAME}={cookie}; Path=/admin/sync-remote; HttpOnly; SameSite=Strict; Max-Age=1800{secure}"
        )).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(OperatorContext {
            csrf,
            cookie: Some(value),
        })
    }
}

/// Refuse unprotected credentials/key endpoints even when diagnostics are open.
#[allow(clippy::too_many_lines)]
pub async fn require_operator(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(credentials) = state.admin_auth.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE,
            "Sync setup requires PERGAMON_ADMIN_USER and PERGAMON_ADMIN_PASSWORD. Local library use is unchanged.")
            .into_response();
    };
    if !credentials.authorizes(request.headers()) {
        return crate::auth::unauthorized();
    }
    let Some(operator) = state.operator.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Sync setup requires a configured PERGAMON_WEB_ORIGIN.",
        )
            .into_response();
    };
    if !operator.destination_matches(request.headers())
        || !operator.source_matches(request.headers())
    {
        return (
            StatusCode::FORBIDDEN,
            "Sync setup origin does not match the configured web origin.",
        )
            .into_response();
    }
    let safe = request.method() == Method::GET || request.method() == Method::HEAD;
    let context =
        match operator.context(request.headers(), safe) {
            Ok(context) => context,
            Err(status) => return (
                status,
                "Operator form session is missing, expired, or unavailable; reload sync settings.",
            )
                .into_response(),
        };
    if !safe {
        if !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| s.starts_with("application/x-www-form-urlencoded"))
        {
            return (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Sync setup requires a protected form submission.",
            )
                .into_response();
        }
        let (parts, body) = request.into_parts();
        let Ok(bytes) = axum::body::to_bytes(body, 65_536).await else {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                "Sync setup form exceeds its size limit.",
            )
                .into_response();
        };
        let tokens: Vec<_> = url::form_urlencoded::parse(&bytes)
            .filter(|(name, _)| name == "_csrf")
            .map(|(_, value)| value.into_owned())
            .collect();
        let header_token = parts
            .headers
            .get("x-csrf-token")
            .and_then(|h| h.to_str().ok());
        let supplied = if tokens.len() == 1 {
            Some(tokens[0].as_str())
        } else if tokens.is_empty() {
            header_token
        } else {
            None
        };
        if !supplied.is_some_and(|value| {
            crate::auth::constant_time_eq(value.as_bytes(), context.csrf.as_bytes())
        }) {
            return (
                StatusCode::FORBIDDEN,
                "Invalid sync setup CSRF token; reload the form.",
            )
                .into_response();
        }
        request = Request::from_parts(parts, Body::from(bytes));
    }
    request.extensions_mut().insert(context.clone());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    // Native form POSTs can send Origin:null under no-referrer; keep only the origin.
    headers.insert("referrer-policy", HeaderValue::from_static("strict-origin"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'",
        ),
    );
    if let Some(cookie) = context.cookie {
        headers.append(header::SET_COOKIE, cookie);
    }
    response
}
