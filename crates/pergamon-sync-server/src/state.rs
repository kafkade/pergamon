// SPDX-License-Identifier: AGPL-3.0-only

//! Application state shared across all request handlers.

use std::sync::Arc;

use crate::error::ApiError;
use crate::fairness::{FairnessConfig, TenantLimiter};
use crate::store::{StoreError, SyncStore};

/// Shared application state available to all request handlers.
///
/// ## Concurrency (WP-3e, #201)
/// The store is **not** wrapped in a process-wide mutex any more. [`SyncStore`]
/// owns one writer connection and a bounded pool of reader connections
/// internally, so concurrent tenants no longer serialize behind a single lock.
///
/// Store calls are blocking `SQLite` work, so handlers must never run them
/// inline on a Tokio worker thread — with a reader pool larger than the worker
/// count, a handful of slow reads would occupy every worker and one heavy tenant
/// could starve the whole runtime. [`AppState::with_store`] and
/// [`AppState::with_tenant_store`] are the only sanctioned way in: they move the
/// work onto `tokio::task::spawn_blocking`, whose pool exists for exactly this.
///
/// One closure is one connection checkout, so a handler that issues two reads
/// keeps them on the same connection.
#[derive(Clone)]
pub struct AppState {
    /// The encrypted event-log and blob store.
    pub store: Arc<SyncStore>,
    /// Per-tenant in-flight concurrency cap (WP-3e, #201), so one heavy tenant
    /// cannot hold every pooled connection.
    pub tenants: Arc<TenantLimiter>,
}

impl AppState {
    /// Wrap a [`SyncStore`] in shared state, deriving the default per-tenant cap
    /// from the store's reader-pool size.
    #[must_use]
    pub fn new(store: SyncStore) -> Self {
        let fairness = FairnessConfig::for_pool(store.read_pool_size(), store.checkout_timeout());
        Self::with_fairness(store, fairness)
    }

    /// Wrap a [`SyncStore`] in shared state with an explicit per-tenant policy.
    #[must_use]
    pub fn with_fairness(store: SyncStore, fairness: FairnessConfig) -> Self {
        Self {
            store: Arc::new(store),
            tenants: Arc::new(TenantLimiter::new(fairness)),
        }
    }

    /// Run a blocking store operation off the async runtime.
    ///
    /// Use this for operations with no tenant of their own. Anything scoped to
    /// an `account_id` should use [`Self::with_tenant_store`] so it is covered by
    /// the per-tenant fairness cap.
    ///
    /// # Errors
    /// Propagates the closure's [`StoreError`] mapped to an [`ApiError`], or a
    /// 500 if the blocking task itself panicked.
    pub async fn with_store<T, F>(&self, op: F) -> Result<T, ApiError>
    where
        F: FnOnce(&SyncStore) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let store = Arc::clone(&self.store);
        match tokio::task::spawn_blocking(move || op(&store)).await {
            Ok(result) => result.map_err(ApiError::from),
            Err(e) => {
                tracing::error!(error = %e, "store task failed");
                Err(ApiError::internal("internal storage error"))
            }
        }
    }

    /// Run a blocking store operation for `account_id`, subject to the
    /// per-tenant concurrency cap.
    ///
    /// The tenant slot is acquired **inside** the blocking task and released
    /// when the closure returns, so it covers exactly the window in which the
    /// tenant holds a database connection.
    ///
    /// # Errors
    /// Returns `503` if the tenant is over its concurrency allowance, propagates
    /// the closure's [`StoreError`], or returns a 500 if the blocking task
    /// panicked.
    pub async fn with_tenant_store<T, F>(&self, account_id: &str, op: F) -> Result<T, ApiError>
    where
        F: FnOnce(&SyncStore) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let store = Arc::clone(&self.store);
        let tenants = Arc::clone(&self.tenants);
        let account_id = account_id.to_owned();
        let joined = tokio::task::spawn_blocking(move || {
            let slot = tenants.acquire(&account_id)?;
            let result = op(&store);
            drop(slot);
            result.map_err(ApiError::from)
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(error = %e, "store task failed");
                Err(ApiError::internal("internal storage error"))
            }
        }
    }
}
