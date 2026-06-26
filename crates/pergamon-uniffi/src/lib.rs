//! # pergamon-uniffi
//!
//! UniFFI facade exposing a deliberately **narrow** slice of [`pergamon_core`]
//! to Apple (Swift / SwiftUI) clients. This crate is the spike deliverable for
//! issue #29: it validates that the zero-I/O Rust core can be consumed natively
//! from iOS via UniFFI-generated Swift bindings.
//!
//! ## Scope
//!
//! Because `pergamon-core` is zero-I/O and there is no SQLite binding on the
//! Apple side yet, this facade serves an **in-memory seeded sample store**. That
//! is enough to demonstrate the two things the spike must prove:
//!
//! - *list*: Swift can receive a `Vec` of records built in Rust, and
//! - *open*: Swift can fetch a single record by id.
//!
//! It also exercises real core logic across the FFI boundary
//! ([`reading_time_from_text`](pergamon_core::reading_time::reading_time_from_text))
//! and real core types (`Uuid`, `OffsetDateTime`, `ContentType`, `DocumentStatus`)
//! which are mapped to FFI-friendly shapes here.
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

// Product/tech names (UniFFI, SwiftUI, SQLite, ...) recur throughout the docs.
#![allow(clippy::doc_markdown)]

use pergamon_core::content_type::ContentType as CoreContentType;
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

/// Build the seeded in-memory corpus of core content items.
///
/// Uses fixed UUIDs and timestamps so `get_item` is deterministic across runs.
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
#[uniffi::export]
#[must_use]
pub fn library_version() -> String {
    pergamon_core::VERSION.to_owned()
}

/// Returns the full seeded list of sample items (the "list" path).
#[uniffi::export]
#[must_use]
pub fn sample_items() -> Vec<ContentItem> {
    seed().iter().map(ContentItem::from).collect()
}

/// Returns sample items filtered to a single triage [`Status`].
#[uniffi::export]
#[must_use]
pub fn items_with_status(status: Status) -> Vec<ContentItem> {
    let core_status: CoreStatus = status.into();
    seed()
        .iter()
        .filter(|item| item.status == core_status)
        .map(ContentItem::from)
        .collect()
}

/// Fetches a single item by its UUID string (the "open" path).
///
/// Returns `None` if the id is malformed or not present in the corpus.
#[uniffi::export]
#[must_use]
#[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
pub fn get_item(id: String) -> Option<ContentItem> {
    let wanted = Uuid::parse_str(&id).ok()?;
    seed()
        .iter()
        .find(|item| item.id == wanted)
        .map(ContentItem::from)
}

/// Estimates reading time in minutes for arbitrary text, delegating to the core
/// reading-time engine. Demonstrates calling pure core logic across the FFI.
#[uniffi::export]
#[must_use]
#[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
pub fn reading_minutes(text: String) -> u32 {
    reading_time_from_text(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_all_sample_items() {
        assert_eq!(sample_items().len(), 5);
    }

    #[test]
    fn filters_by_status() {
        assert_eq!(items_with_status(Status::Archived).len(), 1);
        assert_eq!(items_with_status(Status::Inbox).len(), 1);
        assert!(items_with_status(Status::Discarded).is_empty());
    }

    #[test]
    fn opens_known_item_and_rejects_unknown() {
        let first = &sample_items()[0];
        let fetched = get_item(first.id.clone()).expect("seeded id must resolve");
        assert_eq!(fetched.title, first.title);
        assert!(get_item("not-a-uuid".to_owned()).is_none());
        assert!(get_item(Uuid::from_u128(999).to_string()).is_none());
    }

    #[test]
    fn computes_reading_minutes_via_core() {
        assert_eq!(reading_minutes(String::new()), 0);
        assert!(reading_minutes("word ".repeat(238)) >= 1);
    }

    #[test]
    fn maps_published_at_to_millis() {
        let item = get_item(Uuid::from_u128(1).to_string()).expect("seeded");
        assert_eq!(item.published_at_millis, Some(1_577_836_800_000));
    }
}
