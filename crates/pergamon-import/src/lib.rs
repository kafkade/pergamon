//! Import parsers for external bookmark/read-later services.
//!
//! This crate provides pure parsing functions that convert exported files
//! from services like Raindrop.io, Pocket, Kindle, and Readwise into
//! intermediate Rust structs. The CLI layer handles file I/O and database
//! writes.

pub mod error;
pub mod kindle;
pub mod pocket;
pub mod raindrop;
pub mod readwise;

pub use error::ImportError;
pub use kindle::{KindleClipping, KindleClippingType, parse_kindle_clippings};
pub use pocket::{PocketItem, parse_pocket_html};
pub use raindrop::{RaindropItem, parse_raindrop_csv};
pub use readwise::{ReadwiseItem, parse_readwise_csv};
