//! # pergamon-storage
//!
//! `SQLite` + FTS5 storage adapter for pergamon. Implements the storage
//! traits defined in `pergamon-core` using `rusqlite` with bundled `SQLite`.
//!
//! Responsibilities:
//! - Schema migrations
//! - Content-addressed blob store for raw HTML / PDF / email
//! - Full-text search via FTS5 virtual tables
