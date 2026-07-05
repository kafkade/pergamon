// SPDX-License-Identifier: AGPL-3.0-only

//! # pergamon-sync-server
//!
//! Optional, end-to-end-encrypted multi-device sync server for pergamon.
//!
//! This crate is licensed under **AGPL-3.0**. See the `LICENSE` file in this
//! crate's directory. All client-side pergamon crates are Apache-2.0; per
//! ADR-008 the license boundary is kept clean by keeping server code out of
//! those crates.
//!
//! The server implements the wire contract from ADR-022 and nothing more: it
//! is a **blind, append-only ordering and storage service**. It keeps two
//! per-account stores — an append-only event log and a content-addressed blob
//! store — and understands the structure of neither payload. It orders events
//! on a server-assigned `server_seq`, deduplicates on the client's `change_id`,
//! stores blobs by ciphertext hash, and serves cursors. **It never sees
//! plaintext:** event bodies and blobs are opaque ciphertext, and all payload
//! semantics live in the Apache-2.0 client.
//!
//! ## Endpoints
//! - `GET  /health` — liveness/version.
//! - `POST /v1/blobs/probe` — which ciphertext hashes are missing (dedup probe).
//! - `PUT  /v1/blobs/{account_id}/{ct_hash}` — upload an opaque blob (idempotent).
//! - `GET  /v1/blobs/{account_id}/{ct_hash}` — download an opaque blob.
//! - `POST /v1/events` — push a batch of encrypted event envelopes.
//! - `GET  /v1/events` — pull events with `server_seq > cursor`, ascending.

pub mod envelope;
pub mod error;
pub mod routes;
pub mod state;
pub mod store;

use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

pub use state::AppState;
pub use store::{SyncStore, ct_hash};

/// Build the Axum application router with all routes and middleware.
pub fn build_router(state: AppState) -> Router {
    routes::router(state)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
}
