// SPDX-License-Identifier: AGPL-3.0-only

//! Consistent JSON API error type for the sync server.
//!
//! All error responses share the shape:
//! ```json
//! { "error": "Human-readable message", "code": "ERROR_CODE" }
//! ```

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::store::StoreError;

/// API error that serializes to a consistent JSON format.
#[derive(Debug)]
pub struct ApiError {
    /// HTTP status code.
    pub status: StatusCode,
    /// Human-readable error message.
    pub message: String,
    /// Machine-readable error code.
    pub code: &'static str,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

/// JSON body for error responses.
#[derive(Serialize)]
struct ErrorBody {
    error: String,
    code: &'static str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.message,
            code: self.code,
        };
        (self.status, Json(body)).into_response()
    }
}

impl ApiError {
    /// 400 Bad Request.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            code: "BAD_REQUEST",
        }
    }

    /// 404 Not Found.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
            code: "NOT_FOUND",
        }
    }

    /// 401 Unauthorized. Used as the **uniform** failure for a wrong password
    /// and for an unknown identity alike, so the two are indistinguishable (no
    /// account-existence oracle — design §1.6).
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
            code: "UNAUTHORIZED",
        }
    }

    /// 403 Forbidden — the caller authenticated successfully but is not
    /// authorized for the target tenant (WP-3c, #197). Distinct from
    /// [`Self::unauthorized`]: a 401 means "we could not authenticate you"
    /// (uniform, no account-existence leak), a 403 means "you are authenticated
    /// but this is not your account".
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            code: "FORBIDDEN",
        }
    }

    /// 429 Too Many Requests — per-identity online-guess throttling (design
    /// §1.7). Keyed uniformly on the identity handle, so it does not leak
    /// account existence.
    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: message.into(),
            code: "TOO_MANY_REQUESTS",
        }
    }

    /// 503 Service Unavailable — transient capacity limit (e.g. the pending
    /// login table is saturated).
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
            code: "UNAVAILABLE",
        }
    }

    /// 409 Conflict — used when an event references a blob that has not been
    /// uploaded yet (violating upload-before-commit).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
            code: "CONFLICT",
        }
    }

    /// 507 Insufficient Storage — the caller's write would exceed the account's
    /// configured storage quota (WP-3d, #198). The message names which limit
    /// (bytes or object count) was hit; the code is a stable `QUOTA_EXCEEDED`.
    /// Reads stay allowed while over quota so a tenant can export/delete to
    /// recover.
    pub fn insufficient_storage(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INSUFFICIENT_STORAGE,
            message: message.into(),
            code: "QUOTA_EXCEEDED",
        }
    }

    /// 500 Internal Server Error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            code: "INTERNAL_ERROR",
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::BlobHashMismatch { .. } => Self::bad_request(err.to_string()),
            StoreError::MissingBlob { .. } => Self::conflict(err.to_string()),
            StoreError::QuotaExceeded { .. } => Self::insufficient_storage(err.to_string()),
            StoreError::Db(e) => {
                tracing::error!(error = %e, "store database error");
                Self::internal("internal storage error")
            }
            StoreError::Json(e) => {
                tracing::error!(error = %e, "store blob_refs encoding error");
                Self::internal("internal storage error")
            }
        }
    }
}

impl From<crate::auth::store::AuthStoreError> for ApiError {
    fn from(err: crate::auth::store::AuthStoreError) -> Self {
        use crate::auth::store::AuthStoreError;
        match err {
            AuthStoreError::HandleExists => Self::conflict(err.to_string()),
            AuthStoreError::Db(e) => {
                tracing::error!(error = %e, "auth store database error");
                Self::internal("internal storage error")
            }
        }
    }
}
