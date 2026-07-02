//! # pergamon-uniffi
//!
//! UniFFI facade exposing a deliberately **narrow** slice of [`pergamon_core`]
//! to Apple (Swift / SwiftUI) clients. This crate is the single, exclusive
//! UniFFI export surface for Apple: Swift never links `pergamon-core` or any
//! internal crate directly. It implements the conventions ratified in
//! **ADR-019** (UniFFI boundary and error mapping).
//!
//! ## Exported surface
//!
//! - **Records** ([`ContentItem`]): plain value views of core types.
//! - **Enums** ([`ContentType`], [`Status`]): mirrored discriminators that
//!   decouple the FFI ABI from the internal `pergamon_core` enums.
//! - **Error** ([`PergamonError`]): a single, flat error enum mapped to Swift
//!   `throws`.
//! - **Object handle** ([`Library`]): the stateful entry point the app drives
//!   (`inbox`, `items`, `item`, `search`, ...). Backed by an in-memory seeded
//!   corpus for now; the on-device SQLite store lands with the offline-database
//!   work (#118 / ADR-020).
//! - **Free functions** ([`library_version`], [`reading_minutes`]): stateless
//!   helpers.
//!
//! ## Boundary mapping
//!
//! | Core type         | FFI type (this crate)      |
//! |-------------------|----------------------------|
//! | `Uuid`            | `String`                   |
//! | `OffsetDateTime`  | `i64` (Unix epoch millis)  |
//! | `Option<T>`       | Swift optional             |
//! | `ContentType`     | [`ContentType`] enum        |
//! | `DocumentStatus`  | [`Status`] enum             |
//! | `Result<T, E>`    | Swift `throws` ([`PergamonError`]) |

// Product/tech names (UniFFI, SwiftUI, SQLite, ...) recur throughout the docs.
#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use pergamon_core::content_type::ContentType as CoreContentType;
use pergamon_core::error::CoreError;
use pergamon_core::model::ContentItem as CoreContentItem;
use pergamon_core::reading_time::reading_time_from_text;
use pergamon_core::status::DocumentStatus as CoreStatus;

use time::OffsetDateTime;
use uuid::Uuid;

uniffi::setup_scaffolding!();

/// Content type discriminator, mirroring `pergamon_core::content_type::ContentType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ContentType {
    /// An item ingested from an RSS/Atom feed.
    FeedItem,
    /// A web article captured for reading.
    Article,
    /// A saved bookmark.
    Bookmark,
    /// A user highlight or annotation.
    Highlight,
    /// A PDF document.
    Pdf,
    /// A podcast episode.
    PodcastEpisode,
}

impl From<CoreContentType> for ContentType {
    fn from(value: CoreContentType) -> Self {
        match value {
            CoreContentType::FeedItem => Self::FeedItem,
            CoreContentType::Article => Self::Article,
            CoreContentType::Bookmark => Self::Bookmark,
            CoreContentType::Highlight => Self::Highlight,
            CoreContentType::Pdf => Self::Pdf,
            CoreContentType::PodcastEpisode => Self::PodcastEpisode,
        }
    }
}

/// Lifecycle status in the triage workflow, mirroring
/// `pergamon_core::status::DocumentStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Status {
    /// Newly captured, awaiting triage.
    Inbox,
    /// Marked for later reading.
    Later,
    /// Saved as a reference.
    Reference,
    /// Currently being read.
    Reading,
    /// Finished / processed.
    Archived,
    /// Explicitly discarded.
    Discarded,
}

impl From<CoreStatus> for Status {
    fn from(value: CoreStatus) -> Self {
        match value {
            CoreStatus::Inbox => Self::Inbox,
            CoreStatus::Later => Self::Later,
            CoreStatus::Reference => Self::Reference,
            CoreStatus::Reading => Self::Reading,
            CoreStatus::Archived => Self::Archived,
            CoreStatus::Discarded => Self::Discarded,
        }
    }
}

impl From<Status> for CoreStatus {
    fn from(value: Status) -> Self {
        match value {
            Status::Inbox => Self::Inbox,
            Status::Later => Self::Later,
            Status::Reference => Self::Reference,
            Status::Reading => Self::Reading,
            Status::Archived => Self::Archived,
            Status::Discarded => Self::Discarded,
        }
    }
}

/// An FFI-friendly view of a `pergamon_core::model::ContentItem`.
///
/// `id` is a UUID string and `published_at_millis` is Unix epoch milliseconds,
/// keeping the record trivially representable across the UniFFI boundary.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ContentItem {
    /// Stable UUID, serialized as a string.
    pub id: String,
    /// Title of the content item.
    pub title: String,
    /// URL of the content, if any.
    pub url: Option<String>,
    /// Author or creator, if known.
    pub author: Option<String>,
    /// Content type discriminator.
    pub content_type: ContentType,
    /// Lifecycle status in the triage workflow.
    pub status: Status,
    /// Short excerpt or summary.
    pub excerpt: Option<String>,
    /// Publication time as Unix epoch milliseconds, if known.
    pub published_at_millis: Option<i64>,
    /// Estimated reading time in minutes, computed by the core engine.
    pub reading_minutes: u32,
}

fn millis(dt: OffsetDateTime) -> i64 {
    // nanoseconds since the Unix epoch, narrowed to milliseconds. Any realistic
    // calendar date fits comfortably in i64 milliseconds.
    #[allow(clippy::cast_possible_truncation)]
    let ms = (dt.unix_timestamp_nanos() / 1_000_000) as i64;
    ms
}

impl From<&CoreContentItem> for ContentItem {
    fn from(item: &CoreContentItem) -> Self {
        let reading_minutes = item
            .content_text
            .as_deref()
            .map_or(0, reading_time_from_text);
        Self {
            id: item.id.to_string(),
            title: item.title.clone(),
            url: item.url.clone(),
            author: item.author.clone(),
            content_type: item.content_type.into(),
            status: item.status.into(),
            excerpt: item.excerpt.clone(),
            published_at_millis: item.published_at.map(millis),
            reading_minutes,
        }
    }
}

/// A single, **flat** error type mapped to Swift `throws`, per ADR-019.
///
/// The facade collapses internal crate errors into a small, stable set of
/// categories the app can act on. Each variant carries a human-readable
/// `message`; Swift shows the message and can `switch` on the case. Fine-grained
/// internal variants are intentionally *not* exported — they survive only as the
/// message string, keeping the FFI ABI stable across internal refactors.
#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum PergamonError {
    /// A requested entity does not exist.
    #[error("{message}")]
    NotFound {
        /// Human-readable detail.
        message: String,
    },
    /// Caller-supplied input was malformed or failed validation.
    #[error("{message}")]
    InvalidInput {
        /// Human-readable detail.
        message: String,
    },
    /// An on-device storage operation failed.
    ///
    /// Reserved for the SQLite-backed `Library` (#118); unused while the corpus
    /// is in-memory.
    #[error("{message}")]
    Storage {
        /// Human-readable detail.
        message: String,
    },
    /// A network operation failed.
    ///
    /// Reserved for the orchestration layer that wraps HTTP (never
    /// `pergamon-core`); unused today.
    #[error("{message}")]
    Network {
        /// Human-readable detail.
        message: String,
    },
    /// An unexpected internal error the app cannot act on.
    #[error("{message}")]
    Internal {
        /// Human-readable detail.
        message: String,
    },
}

impl From<CoreError> for PergamonError {
    fn from(err: CoreError) -> Self {
        match err {
            // Every current `CoreError` variant is a parse/validation failure of
            // caller-controlled input, so they map to `InvalidInput`. The
            // exhaustive match makes a new core variant a compile error here.
            CoreError::UnknownContentType(_)
            | CoreError::UnknownDocumentStatus(_)
            | CoreError::UnknownCardState(_) => Self::InvalidInput {
                message: err.to_string(),
            },
        }
    }
}

/// Build the seeded in-memory corpus of core content items.
///
/// Uses fixed UUIDs and timestamps so [`Library::item`] is deterministic across runs.
fn seed() -> Vec<CoreContentItem> {
    fn at(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }

    #[allow(clippy::too_many_arguments)]
    fn item(
        n: u128,
        title: &str,
        url: &str,
        author: Option<&str>,
        content_type: CoreContentType,
        status: CoreStatus,
        excerpt: &str,
        text: &str,
        published: i64,
    ) -> CoreContentItem {
        let created = at(published + 60);
        CoreContentItem {
            id: Uuid::from_u128(n),
            url: Some(url.to_owned()),
            title: title.to_owned(),
            author: author.map(ToOwned::to_owned),
            content_type,
            status,
            content_text: Some(text.to_owned()),
            excerpt: Some(excerpt.to_owned()),
            published_at: Some(at(published)),
            created_at: created,
            updated_at: created,
            read_at: if status == CoreStatus::Archived {
                Some(at(published + 7200))
            } else {
                None
            },
        }
    }

    let lorem = "word ".repeat(620);
    vec![
        item(
            1,
            "Local-first software: you own your data",
            "https://www.inkandswitch.com/local-first/",
            Some("Ink & Switch"),
            CoreContentType::Article,
            CoreStatus::Inbox,
            "Seven ideals for software that keeps your data on your own devices.",
            &lorem,
            1_577_836_800,
        ),
        item(
            2,
            "Designing a spaced-repetition scheduler with FSRS",
            "https://example.org/fsrs-deep-dive",
            Some("A. Researcher"),
            CoreContentType::Article,
            CoreStatus::Later,
            "How the Free Spaced Repetition Scheduler models memory stability.",
            &"word ".repeat(1400),
            1_609_459_200,
        ),
        item(
            3,
            "The Rust + UniFFI mobile toolchain",
            "https://example.org/rust-uniffi-mobile",
            Some("M. Mobile"),
            CoreContentType::FeedItem,
            CoreStatus::Reading,
            "Sharing a Rust core across iOS and Android without hand-written FFI.",
            &"word ".repeat(300),
            1_640_995_200,
        ),
        item(
            4,
            "pergamon roadmap notes",
            "https://example.org/pergamon-notes.pdf",
            None,
            CoreContentType::Pdf,
            CoreStatus::Reference,
            "Working notes captured as a PDF for later reference.",
            &"word ".repeat(90),
            1_672_531_200,
        ),
        item(
            5,
            "Why I switched from Inoreader",
            "https://example.org/switching",
            Some("Power User"),
            CoreContentType::Bookmark,
            CoreStatus::Archived,
            "A migration story toward a unified, local-first reading workflow.",
            &"word ".repeat(210),
            1_704_067_200,
        ),
    ]
}

/// Returns the version of the underlying `pergamon-core` library.
///
/// A stateless helper that needs no [`Library`] handle.
#[uniffi::export]
#[must_use]
pub fn library_version() -> String {
    pergamon_core::VERSION.to_owned()
}

/// Estimates reading time in minutes for arbitrary text, delegating to the core
/// reading-time engine. Demonstrates calling pure core logic across the FFI.
///
/// A stateless helper that needs no [`Library`] handle.
#[uniffi::export]
#[must_use]
#[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
pub fn reading_minutes(text: String) -> u32 {
    reading_time_from_text(&text)
}

/// The stateful entry point the app drives, per ADR-019.
///
/// `Library` is a `#[uniffi::export]` object handle: Swift holds it as a
/// reference type (`Arc`), and its methods are the primary way the app reads the
/// core. It owns interior state behind `Send + Sync` (an immutable seeded corpus
/// for now), so its methods are safe to call from any thread. The on-device
/// SQLite store replaces the seed with the offline-database work (#118 /
/// ADR-020); the method surface is designed to absorb that change additively.
///
/// Calls are **synchronous and blocking** by design (ADR-019): core logic and
/// future local-DB access do not wait on anything, so the app invokes these off
/// the main actor rather than paying for `async`.
#[derive(uniffi::Object)]
pub struct Library {
    items: Vec<CoreContentItem>,
}

#[uniffi::export]
impl Library {
    /// Opens a library backed by the built-in seeded corpus.
    ///
    /// Deterministic (fixed UUIDs and timestamps) so lookups are stable across
    /// runs and tests.
    #[uniffi::constructor]
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { items: seed() })
    }

    /// Returns every item in triage-`Inbox` status (the primary landing screen).
    #[must_use]
    pub fn inbox(&self) -> Vec<ContentItem> {
        self.items_with_status(Status::Inbox)
    }

    /// Returns all items in the library (the "list" path).
    #[must_use]
    pub fn items(&self) -> Vec<ContentItem> {
        self.items.iter().map(ContentItem::from).collect()
    }

    /// Returns items filtered to a single triage [`Status`].
    #[must_use]
    pub fn items_with_status(&self, status: Status) -> Vec<ContentItem> {
        let core_status: CoreStatus = status.into();
        self.items
            .iter()
            .filter(|item| item.status == core_status)
            .map(ContentItem::from)
            .collect()
    }

    /// Fetches a single item by its UUID string (the "open" path).
    ///
    /// # Errors
    ///
    /// Returns [`PergamonError::InvalidInput`] if `id` is not a valid UUID, and
    /// [`PergamonError::NotFound`] if no item with that id exists. This exercises
    /// the ADR-019 error mapping across the FFI boundary (Swift `throws`).
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn item(&self, id: String) -> Result<ContentItem, PergamonError> {
        let wanted = Uuid::parse_str(&id).map_err(|_| PergamonError::InvalidInput {
            message: format!("not a valid UUID: {id}"),
        })?;
        self.items
            .iter()
            .find(|item| item.id == wanted)
            .map(ContentItem::from)
            .ok_or(PergamonError::NotFound {
                message: format!("no item with id {id}"),
            })
    }

    /// Returns items whose title, author, excerpt, or URL contains `query`
    /// (case-insensitive). An empty query matches nothing.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn search(&self, query: String) -> Vec<ContentItem> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let hit = |field: Option<&String>| {
            field.is_some_and(|value| value.to_lowercase().contains(&needle))
        };
        self.items
            .iter()
            .filter(|item| {
                item.title.to_lowercase().contains(&needle)
                    || hit(item.author.as_ref())
                    || hit(item.excerpt.as_ref())
                    || hit(item.url.as_ref())
            })
            .map(ContentItem::from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Arc<Library> {
        Library::new()
    }

    #[test]
    fn lists_all_items() {
        assert_eq!(library().items().len(), 5);
    }

    #[test]
    fn filters_by_status() {
        let lib = library();
        assert_eq!(lib.items_with_status(Status::Archived).len(), 1);
        assert_eq!(lib.items_with_status(Status::Inbox).len(), 1);
        assert!(lib.items_with_status(Status::Discarded).is_empty());
    }

    #[test]
    fn inbox_returns_only_inbox_items() {
        let inbox = library().inbox();
        assert_eq!(inbox.len(), 1);
        assert!(inbox.iter().all(|item| item.status == Status::Inbox));
    }

    #[test]
    fn opens_known_item() {
        let lib = library();
        let first = &lib.items()[0];
        let fetched = lib.item(first.id.clone()).expect("seeded id must resolve");
        assert_eq!(fetched.title, first.title);
    }

    #[test]
    fn open_rejects_malformed_id_as_invalid_input() {
        match library().item("not-a-uuid".to_owned()) {
            Err(PergamonError::InvalidInput { .. }) => {}
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn open_reports_unknown_id_as_not_found() {
        match library().item(Uuid::from_u128(999).to_string()) {
            Err(PergamonError::NotFound { .. }) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn search_matches_title_and_author_case_insensitively() {
        let lib = library();
        assert_eq!(lib.search("inoreader".to_owned()).len(), 1);
        assert_eq!(lib.search("RESEARCHER".to_owned()).len(), 1);
        assert!(lib.search("   ".to_owned()).is_empty());
        assert!(lib.search("no-such-content".to_owned()).is_empty());
    }

    #[test]
    fn computes_reading_minutes_via_core() {
        assert_eq!(reading_minutes(String::new()), 0);
        assert!(reading_minutes("word ".repeat(238)) >= 1);
    }

    #[test]
    fn maps_published_at_to_millis() {
        let item = library()
            .item(Uuid::from_u128(1).to_string())
            .expect("seeded");
        assert_eq!(item.published_at_millis, Some(1_577_836_800_000));
    }

    #[test]
    fn maps_core_error_to_invalid_input() {
        let err: PergamonError = CoreError::UnknownContentType("bogus".to_owned()).into();
        assert!(matches!(err, PergamonError::InvalidInput { .. }));
    }
}
