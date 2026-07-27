// SPDX-License-Identifier: Apache-2.0

//! Transport authentication credentials (issue #183).
//!
//! The blind-relay sync server has no built-in authentication; the documented
//! secure self-hosting deployment ([`docs/sync-server.md`]) puts it behind a
//! reverse proxy that enforces HTTP Basic auth. This type lets a caller supply
//! that credential as a first-class `Authorization` header value instead of
//! embedding it in the server URL (which leaks into logs, process lists, and the
//! persisted `server_url`).
//!
//! It lives outside the `http` feature so callers can construct it even when the
//! `reqwest`-backed transports are not compiled in. The secret is never included
//! in the [`Debug`] representation, and the transports mark the resulting header
//! sensitive so `reqwest` never records it.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// A credential the HTTP transports send as an `Authorization` header.
///
/// Construct it from configuration (e.g. environment variables) and hand it to
/// [`crate::http::HttpTransport::with_credential`] or
/// [`crate::http_relay::HttpRelay::with_credential`]. The secret is redacted in
/// [`Debug`] output so it cannot leak through logging.
#[derive(Clone)]
pub enum TransportCredential {
    /// HTTP Basic auth: sent as `Basic <base64(username:password)>`.
    Basic {
        /// The Basic-auth username (before the `:` separator).
        username: String,
        /// The Basic-auth password (after the `:` separator).
        password: String,
    },
    /// Bearer-token auth: sent as `Bearer <token>`.
    Bearer {
        /// The opaque bearer token.
        token: String,
    },
}

impl TransportCredential {
    /// The exact `Authorization` header value this credential produces.
    ///
    /// - [`TransportCredential::Basic`] yields `Basic <base64(username:":"password)>`
    ///   using the standard (padded) base64 alphabet, per RFC 7617.
    /// - [`TransportCredential::Bearer`] yields `Bearer <token>`.
    #[must_use]
    pub fn authorization_header_value(&self) -> String {
        match self {
            Self::Basic { username, password } => {
                let encoded = STANDARD.encode(format!("{username}:{password}"));
                format!("Basic {encoded}")
            }
            Self::Bearer { token } => format!("Bearer {token}"),
        }
    }
}

/// Redacts the secret so it never leaks through `{:?}` logging.
impl std::fmt::Debug for TransportCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Basic { username, .. } => f
                .debug_struct("TransportCredential::Basic")
                .field("username", username)
                .field("password", &"<redacted>")
                .finish(),
            Self::Bearer { .. } => f
                .debug_struct("TransportCredential::Bearer")
                .field("token", &"<redacted>")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_header_value_is_rfc7617_base64() {
        let cred = TransportCredential::Basic {
            username: "aladdin".to_owned(),
            password: "opensesame".to_owned(),
        };
        // base64("aladdin:opensesame") == "YWxhZGRpbjpvcGVuc2VzYW1l"
        assert_eq!(
            cred.authorization_header_value(),
            "Basic YWxhZGRpbjpvcGVuc2VzYW1l"
        );
    }

    #[test]
    fn bearer_header_value_is_verbatim_token() {
        let cred = TransportCredential::Bearer {
            token: "s3cr3t-token".to_owned(),
        };
        assert_eq!(cred.authorization_header_value(), "Bearer s3cr3t-token");
    }

    #[test]
    fn debug_redacts_basic_password() {
        let cred = TransportCredential::Basic {
            username: "youruser".to_owned(),
            password: "hunter2".to_owned(),
        };
        let rendered = format!("{cred:?}");
        assert!(rendered.contains("youruser"));
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn debug_redacts_bearer_token() {
        let cred = TransportCredential::Bearer {
            token: "super-secret-token".to_owned(),
        };
        let rendered = format!("{cred:?}");
        assert!(!rendered.contains("super-secret-token"));
        assert!(rendered.contains("<redacted>"));
    }
}
