// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP Basic authentication for the admin diagnostics routes.
//!
//! Admin routes are gated by optional credentials supplied via the
//! `--admin-user` / `--admin-password` flags (or the `PERGAMON_ADMIN_USER` /
//! `PERGAMON_ADMIN_PASSWORD` environment variables). When no credentials are
//! configured the admin routes stay open — appropriate for a local-first,
//! single-user deployment — and the server logs a warning at startup.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;

use crate::state::AppState;

/// Admin credentials for HTTP Basic authentication.
#[derive(Clone)]
pub struct AdminCredentials {
    user: String,
    password: String,
}

impl AdminCredentials {
    /// Create credentials from a username and password.
    #[must_use]
    pub const fn new(user: String, password: String) -> Self {
        Self { user, password }
    }

    /// Check whether the supplied username and password match, comparing in
    /// (near) constant time to avoid leaking length/positional information.
    #[must_use]
    fn matches(&self, user: &str, password: &str) -> bool {
        constant_time_eq(self.user.as_bytes(), user.as_bytes())
            & constant_time_eq(self.password.as_bytes(), password.as_bytes())
    }
}

/// Constant-time byte-slice equality. Returns `true` only when both slices have
/// the same length and contents. The comparison always inspects every byte of
/// the longer slice so timing does not reveal where a mismatch occurred.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = u8::from(a.len() != b.len());
    let max = a.len().max(b.len());
    for i in 0..max {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

/// Parse a `username:password` pair from an `Authorization: Basic …` header.
fn parse_basic_auth(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, password) = decoded.split_once(':')?;
    Some((user.to_owned(), password.to_owned()))
}

/// Build the `401 Unauthorized` challenge response.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"pergamon admin\", charset=\"UTF-8\"",
        )],
        "401 Unauthorized — admin credentials required",
    )
        .into_response()
}

/// Axum middleware that enforces admin Basic authentication.
///
/// When no admin credentials are configured the request passes through. When
/// credentials are configured, the request must supply a matching
/// `Authorization: Basic` header or it receives `401 Unauthorized`.
pub async fn require_admin_auth(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(creds) = state.admin_auth.as_ref() else {
        return next.run(req).await;
    };

    match parse_basic_auth(req.headers()) {
        Some((user, password)) if creds.matches(&user, &password) => next.run(req).await,
        _ => unauthorized(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn constant_time_eq_matches() {
        assert!(constant_time_eq(b"hunter2", b"hunter2"));
        assert!(!constant_time_eq(b"hunter2", b"hunter3"));
        assert!(!constant_time_eq(b"short", b"longer-value"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn credentials_match() {
        let creds = AdminCredentials::new("admin".into(), "s3cret".into());
        assert!(creds.matches("admin", "s3cret"));
        assert!(!creds.matches("admin", "wrong"));
        assert!(!creds.matches("root", "s3cret"));
    }

    #[test]
    fn parse_basic_auth_decodes() {
        let mut headers = HeaderMap::new();
        // base64("admin:s3cret") = YWRtaW46czNjcmV0
        headers.insert(
            header::AUTHORIZATION,
            "Basic YWRtaW46czNjcmV0".parse().unwrap(),
        );
        let (user, password) = parse_basic_auth(&headers).unwrap();
        assert_eq!(user, "admin");
        assert_eq!(password, "s3cret");
    }

    #[test]
    fn parse_basic_auth_rejects_garbage() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert!(parse_basic_auth(&headers).is_none());
    }
}
