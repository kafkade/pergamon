//! Import parsers for external bookmark/read-later services.
//!
//! This crate provides pure parsing functions that convert exported files
//! from services like Raindrop.io and Pocket into intermediate Rust structs.
//! The CLI layer handles file I/O and database writes.

pub mod error;
pub mod pocket;
pub mod raindrop;

pub use error::ImportError;
pub use pocket::{PocketItem, parse_pocket_html};
pub use raindrop::{RaindropItem, parse_raindrop_csv};
