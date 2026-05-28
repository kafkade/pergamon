//! # pergamon-feed
//!
//! RSS/Atom/JSON Feed parsing, OPML import/export, and feed discovery.
//!
//! This crate handles:
//! - Parsing RSS 2.0, Atom 1.0, and JSON Feed 1.1 formats (via `feed-rs`)
//! - OPML import and export for subscription lists
//! - Feed URL discovery from web page `<link>` tags

mod error;
pub mod opml;
mod parser;

pub use error::FeedError;
pub use opml::{OpmlDocument, OpmlOutline, count_outlines, generate_opml, parse_opml};
pub use parser::{ParsedEntry, ParsedFeed, parse_feed};
