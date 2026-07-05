// SPDX-License-Identifier: Apache-2.0

//! # pergamon-sync
//!
//! The **client-side sync engine** for pergamon's optional, end-to-end-encrypted
//! multi-device sync (issue #126). It is Apache-2.0 and never links the AGPL
//! `pergamon-sync-server`; it speaks that server's ADR-022 wire protocol over an
//! abstract [`Transport`], encrypts every event body with `pergamon-crypto`
//! (ADR-024), and resolves concurrent edits with the ADR-023 conflict policy in
//! `pergamon-core::sync`.
//!
//! ## Responsibilities
//!
//! - **Push**: drain the local outbox (`pergamon-storage`), upload any blobs the
//!   pending events reference, encrypt each [`ChangeBody`] into an
//!   [`wire::EventInput`], append the batch to the server, and mark the outbox
//!   rows acknowledged.
//! - **Pull**: fetch events with `server_seq` past the local cursor, suppress
//!   this device's own echoes, decrypt each body, fetch referenced blobs, apply
//!   the change through the ADR-023 merge policy, and advance the cursor — all
//!   idempotently, so re-pulling a page is a no-op.
//!
//! ## Layers
//!
//! - [`wire`] — a serde mirror of the server's ADR-022 frame types.
//! - [`transport`] — the [`Transport`] trait plus an in-memory test double.
//! - [`crypto`] — the encryption glue: build headers, encrypt/decrypt events and
//!   blobs, and blind `entity_ref`s.
//! - [`engine`] — the [`SyncEngine`]: push, pull, and a combined sync round.
//! - [`apply`] — mapping a decrypted [`ChangeBody`] onto merged storage writes.
//! - [`blob`] — the client blob-plaintext store trait used for blob sync.

#![forbid(unsafe_code)]

pub mod apply;
pub mod blob;
pub mod crypto;
pub mod engine;
pub mod error;
pub mod transport;
pub mod wire;

#[cfg(feature = "http")]
pub mod http;

pub use blob::{BlobStore, MemoryBlobStore};
pub use crypto::CryptoContext;
pub use engine::{SyncEngine, SyncStats};
pub use error::SyncError;
pub use transport::{MemoryTransport, Transport};

#[doc(inline)]
pub use pergamon_core::sync::event::ChangeBody;
