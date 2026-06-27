// SPDX-License-Identifier: AGPL-3.0-only

//! Application state shared across all request handlers.

use std::sync::Arc;

use pergamon_storage::Database;

use crate::auth::AdminCredentials;

/// Shared application state available to all request handlers.
///
/// Uses `std::sync::Mutex` because database operations are blocking and
/// fast — the lock is never held across `.await` points. This is adequate
/// for a single-user server; a connection pool may be introduced later if
/// concurrent workload grows.
#[derive(Clone)]
pub struct AppState {
    /// Database handle shared across request handlers.
    pub db: Arc<std::sync::Mutex<Database>>,
    /// HTTP client for fetching URLs (save, feed subscribe/sync).
    pub http: reqwest::Client,
    /// Optional admin credentials. When `None`, the admin routes are open.
    pub admin_auth: Option<AdminCredentials>,
}
