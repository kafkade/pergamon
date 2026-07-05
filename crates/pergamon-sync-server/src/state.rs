// SPDX-License-Identifier: AGPL-3.0-only

//! Application state shared across all request handlers.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::ApiError;
use crate::store::SyncStore;

/// Shared application state available to all request handlers.
///
/// The store is guarded by a `std::sync::Mutex` because its operations are
/// blocking and fast; the lock is never held across an `.await`. This is
/// adequate for a single-node sync server — a connection pool may be
/// introduced later if concurrent workload grows.
#[derive(Clone)]
pub struct AppState {
    /// The encrypted event-log and blob store.
    pub store: Arc<Mutex<SyncStore>>,
}

impl AppState {
    /// Wrap a [`SyncStore`] in shared, lockable state.
    #[must_use]
    pub fn new(store: SyncStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// Lock the store, mapping a poisoned lock to a 500 error.
    ///
    /// # Errors
    /// Returns [`ApiError::internal`] if the mutex has been poisoned.
    pub fn lock_store(&self) -> Result<MutexGuard<'_, SyncStore>, ApiError> {
        self.store
            .lock()
            .map_err(|_| ApiError::internal("store lock poisoned"))
    }
}
