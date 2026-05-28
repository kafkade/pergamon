//! # pergamon-storage
//!
//! `SQLite` + FTS5 storage adapter for pergamon. Implements the storage
//! traits defined in `pergamon-core` using `rusqlite` with bundled `SQLite`.
//!
//! Responsibilities:
//! - Schema migrations (via `refinery`)
//! - CRUD operations for the unified content model
//! - Full-text search via FTS5 virtual tables

pub mod db;
pub mod error;

pub use db::ContentItemFilter;
pub use db::Database;
pub use db::SearchFilter;
pub use error::StorageError;
