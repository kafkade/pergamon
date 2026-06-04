// SPDX-License-Identifier: AGPL-3.0-only

//! Consistent API error type.
//!
//! All error responses follow the format:
//! ```json
//! { "error": "Human-readable message", "code": "ERROR_CODE" }
//! ```

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

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

    /// 409 Conflict (duplicate).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
            code: "CONFLICT",
        }
    }

    /// 422 Unprocessable Entity (extraction/parse failure).
    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: message.into(),
            code: "UNPROCESSABLE_ENTITY",
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

    /// 502 Bad Gateway (upstream fetch failure).
    pub fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
            code: "BAD_GATEWAY",
        }
    }
}

impl From<pergamon_storage::StorageError> for ApiError {
    fn from(err: pergamon_storage::StorageError) -> Self {
        let msg = err.to_string();
        // Map known storage errors to appropriate HTTP status codes.
        if msg.contains("not found") {
            Self::not_found(msg)
        } else if msg.contains("UNIQUE constraint") || msg.contains("already exists") {
            Self::conflict(msg)
        } else {
            tracing::error!(error = %err, "storage error");
            Self::internal("internal storage error")
        }
    }
}
