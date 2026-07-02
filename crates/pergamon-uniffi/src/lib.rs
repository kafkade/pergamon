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
//!   (`inbox`, `items`, `item`, `search`, and triage mutations `mark_read`,
//!   `archive`, `save_for_later`, ...). Backed by an in-memory seeded corpus for
//!   now; the on-device SQLite store lands with the offline-database work
//!   (#118 / ADR-020).
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

use std::sync::{Arc, Mutex, PoisonError};

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

/// An FFI-friendly view of a user-defined tag, mirroring
/// `pergamon_core::model::Tag`.
///
/// Tags are matched case-insensitively but keep their first-seen display form.
/// `id` is a UUID string, keeping the record trivially representable across the
/// UniFFI boundary.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Tag {
    /// Stable UUID, serialized as a string.
    pub id: String,
    /// Display name (first-seen form; deduplicated case-insensitively).
    pub name: String,
    /// Number of items currently carrying this tag.
    pub item_count: u32,
}

/// An FFI-friendly view of a hierarchical collection, mirroring
/// `pergamon_core::model::Collection`.
///
/// Collections can nest via `parent_id`. `depth` is the 0-based nesting level
/// (a root collection has depth 0), precomputed so the app can render an
/// indented tree without walking the parent chain itself.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Collection {
    /// Stable UUID, serialized as a string.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Parent collection UUID string, or `None` for a root collection.
    pub parent_id: Option<String>,
    /// Number of items directly in this collection.
    pub item_count: u32,
    /// 0-based nesting depth (root = 0), precomputed for indented rendering.
    pub depth: u32,
}

/// Optional, AND-combined facets that narrow a [`Library::search_filtered`]
/// query, mirroring the CLI/web search facets (`type / tag / status / source /
/// since / before`) so results stay consistent across clients on the same
/// library.
///
/// Every field is optional; `None` (or an empty `tag`/`source`) leaves that
/// facet unconstrained. Dates are Unix epoch milliseconds and filter on the
/// item's publication time.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchFacets {
    /// Restrict to a single content type.
    pub content_type: Option<ContentType>,
    /// Restrict to a single triage status.
    pub status: Option<Status>,
    /// Restrict to items carrying this tag (case-insensitive).
    pub tag: Option<String>,
    /// Restrict to items from this feed/source name.
    pub source: Option<String>,
    /// Only items published on or after this instant (epoch millis).
    pub since_millis: Option<i64>,
    /// Only items published strictly before this instant (epoch millis).
    pub before_millis: Option<i64>,
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
    /// Normalized extracted body text, used to render the offline reader. `None`
    /// when the item has no extracted content yet.
    pub content_text: Option<String>,
    /// Feed / source name this item was captured from, if any. Drives the
    /// inbox's feed filter.
    pub source_name: Option<String>,
    /// Publication time as Unix epoch milliseconds, if known.
    pub published_at_millis: Option<i64>,
    /// When the item was marked read, as Unix epoch milliseconds. `None` means
    /// unread.
    pub read_at_millis: Option<i64>,
    /// Estimated reading time in minutes, computed by the core engine.
    pub reading_minutes: u32,
    /// Display names of the tags currently assigned to this item.
    pub tags: Vec<String>,
    /// UUID strings of the collections this item currently belongs to.
    pub collection_ids: Vec<String>,
}

fn millis(dt: OffsetDateTime) -> i64 {
    // nanoseconds since the Unix epoch, narrowed to milliseconds. Any realistic
    // calendar date fits comfortably in i64 milliseconds.
    #[allow(clippy::cast_possible_truncation)]
    let ms = (dt.unix_timestamp_nanos() / 1_000_000) as i64;
    ms
}

/// An internal seed row: a core content item plus the FFI-only feed/source name
/// it was captured from, the tags assigned to it, and the collections it belongs
/// to. These organization fields live here (not on `pergamon_core`) so the
/// facade can offer tagging, collections, and feed filtering without changing
/// the core model.
#[derive(Debug, Clone)]
struct Row {
    item: CoreContentItem,
    source: Option<String>,
    /// Display names of the tags on this item (deduplicated case-insensitively).
    tags: Vec<String>,
    /// Ids of the collections this item belongs to.
    collection_ids: Vec<Uuid>,
}

/// An internal tag registry entry. Tags are matched case-insensitively via
/// [`normalize_tag`] but keep their first-seen display form in `name`.
#[derive(Debug, Clone)]
struct TagRow {
    id: Uuid,
    name: String,
}

/// An internal collection registry entry mirroring the nesting fields of
/// `pergamon_core::model::Collection` the facade cares about.
#[derive(Debug, Clone)]
struct CollectionRow {
    id: Uuid,
    name: String,
    parent_id: Option<Uuid>,
}

/// The facade's in-memory state: content rows plus the tag and collection
/// registries. Guarded by a single `Mutex` so multi-entity mutations (adding a
/// tag both registers it and stamps the row) stay consistent without lock
/// ordering concerns.
#[derive(Debug, Clone)]
struct Store {
    rows: Vec<Row>,
    tags: Vec<TagRow>,
    collections: Vec<CollectionRow>,
}

/// Case-insensitive matching key for a tag name: trimmed and lowercased.
fn normalize_tag(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Parses a UUID string, mapping a malformed value to
/// [`PergamonError::InvalidInput`].
fn parse_uuid(id: &str) -> Result<Uuid, PergamonError> {
    Uuid::parse_str(id).map_err(|_| PergamonError::InvalidInput {
        message: format!("not a valid UUID: {id}"),
    })
}

/// Whether a row matches the (already lowercased) search `needle` across title,
/// author, excerpt, URL, and extracted content. An empty needle matches every
/// row (facets do the narrowing in that case).
fn text_matches(row: &Row, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let item = &row.item;
    let hit = |field: Option<&String>| field.is_some_and(|v| v.to_lowercase().contains(needle));
    item.title.to_lowercase().contains(needle)
        || hit(item.author.as_ref())
        || hit(item.excerpt.as_ref())
        || hit(item.url.as_ref())
        || hit(item.content_text.as_ref())
}

/// Whether a row satisfies every active facet (AND-combined).
fn facets_match(row: &Row, facets: &SearchFacets) -> bool {
    let item = &row.item;
    if facets
        .content_type
        .is_some_and(|ct| ContentType::from(item.content_type) != ct)
    {
        return false;
    }
    if facets
        .status
        .is_some_and(|status| Status::from(item.status) != status)
    {
        return false;
    }
    if let Some(tag) = facets.tag.as_ref().filter(|t| !t.trim().is_empty()) {
        let key = normalize_tag(tag);
        if !row.tags.iter().any(|t| normalize_tag(t) == key) {
            return false;
        }
    }
    if facets
        .source
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .is_some_and(|source| row.source.as_deref() != Some(source.as_str()))
    {
        return false;
    }
    date_range_matches(item.published_at.map(millis), facets)
}

/// Whether an item's publication time (epoch millis) satisfies the optional
/// `since`/`before` facet window. `since` is inclusive, `before` exclusive. An
/// item with no publication time fails any active date facet.
fn date_range_matches(published: Option<i64>, facets: &SearchFacets) -> bool {
    if facets.since_millis.is_none() && facets.before_millis.is_none() {
        return true;
    }
    let Some(published) = published else {
        return false;
    };
    if facets.since_millis.is_some_and(|since| published < since) {
        return false;
    }
    if facets
        .before_millis
        .is_some_and(|before| published >= before)
    {
        return false;
    }
    true
}

/// The 0-based nesting depth of a collection (root = 0), walking `parent_id`.
/// Guards against cycles by capping at the number of collections.
fn collection_depth(collections: &[CollectionRow], id: Uuid) -> u32 {
    let mut depth = 0u32;
    let mut current = collections
        .iter()
        .find(|c| c.id == id)
        .and_then(|c| c.parent_id);
    let mut guard = collections.len();
    while let Some(parent) = current {
        depth += 1;
        if guard == 0 {
            break;
        }
        guard -= 1;
        current = collections
            .iter()
            .find(|c| c.id == parent)
            .and_then(|c| c.parent_id);
    }
    depth
}

/// Builds the FFI collection list in depth-first tree order (each parent
/// immediately followed by its children), with per-collection item counts and
/// precomputed depths for indented rendering.
fn ordered_collections(store: &Store) -> Vec<Collection> {
    fn item_count(store: &Store, id: Uuid) -> u32 {
        let count = store
            .rows
            .iter()
            .filter(|row| row.collection_ids.contains(&id))
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    fn push_children(store: &Store, parent: Option<Uuid>, depth: u32, out: &mut Vec<Collection>) {
        let mut children: Vec<&CollectionRow> = store
            .collections
            .iter()
            .filter(|c| c.parent_id == parent)
            .collect();
        children.sort_by_key(|c| c.name.to_lowercase());
        for child in children {
            out.push(Collection {
                id: child.id.to_string(),
                name: child.name.clone(),
                parent_id: child.parent_id.map(|p| p.to_string()),
                item_count: item_count(store, child.id),
                depth,
            });
            push_children(store, Some(child.id), depth + 1, out);
        }
    }

    let mut out = Vec::with_capacity(store.collections.len());
    push_children(store, None, 0, &mut out);
    out
}

impl ContentItem {
    /// Builds the FFI view from a seed [`Row`], folding in the FFI-only source
    /// name alongside the core fields.
    fn from_row(row: &Row) -> Self {
        let item = &row.item;
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
            content_text: item.content_text.clone(),
            source_name: row.source.clone(),
            published_at_millis: item.published_at.map(millis),
            read_at_millis: item.read_at.map(millis),
            reading_minutes,
            tags: row.tags.clone(),
            collection_ids: row.collection_ids.iter().map(Uuid::to_string).collect(),
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

/// Build the seeded in-memory [`Store`]: content rows plus the tag and
/// collection registries.
///
/// Uses fixed UUIDs and timestamps so [`Library::item`] is deterministic across runs.
#[allow(clippy::too_many_lines)] // a flat, readable table of seed rows
fn seed() -> Store {
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
        source: Option<&str>,
    ) -> Row {
        let created = at(published + 60);
        let core = CoreContentItem {
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
        };
        Row {
            item: core,
            source: source.map(ToOwned::to_owned),
            tags: Vec::new(),
            collection_ids: Vec::new(),
        }
    }

    // Fixed collection ids (offset from the item id space to avoid collisions).
    let reading_list = Uuid::from_u128(101);
    let deep_dives = Uuid::from_u128(102);
    let tech = Uuid::from_u128(103);

    let collections = vec![
        CollectionRow {
            id: reading_list,
            name: "Reading List".to_owned(),
            parent_id: None,
        },
        CollectionRow {
            id: deep_dives,
            name: "Deep Dives".to_owned(),
            parent_id: Some(reading_list),
        },
        CollectionRow {
            id: tech,
            name: "Tech".to_owned(),
            parent_id: None,
        },
    ];

    // The tag registry keeps a first-seen display form; membership on rows uses
    // the same display strings.
    let tags = ["local-first", "reading", "memory", "rust", "ios"]
        .iter()
        .enumerate()
        .map(|(i, name)| TagRow {
            id: Uuid::from_u128(201 + i as u128),
            name: (*name).to_owned(),
        })
        .collect();

    let lorem = "word ".repeat(620);
    let mut rows = vec![
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
            Some("Ink & Switch"),
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
            Some("Memory Weekly"),
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
            Some("Rust Mobile Weekly"),
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
            None,
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
            Some("Reader Diaries"),
        ),
    ];

    // Seed tag and collection membership across the corpus.
    rows[0].tags = vec!["local-first".to_owned(), "reading".to_owned()];
    rows[0].collection_ids = vec![reading_list];
    rows[1].tags = vec!["memory".to_owned(), "reading".to_owned()];
    rows[1].collection_ids = vec![deep_dives];
    rows[2].tags = vec!["rust".to_owned(), "ios".to_owned()];
    rows[2].collection_ids = vec![tech];
    rows[4].tags = vec!["reading".to_owned()];
    rows[4].collection_ids = vec![reading_list];

    Store {
        rows,
        tags,
        collections,
    }
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
/// reference type (`Arc`), and its methods are the primary way the app reads and
/// triages the core. It owns interior state behind `Send + Sync` — a seeded
/// corpus guarded by a `Mutex`, so reads (`inbox`, `items`, ...) and triage
/// mutations (`mark_read`, `archive`, `save_for_later`, ...) are safe to call
/// from any thread. Mutations are visible within the process session; the
/// on-device SQLite store replaces the seed with the offline-database work
/// (#118 / ADR-020), persisting them across launches behind this same surface.
///
/// Calls are **synchronous and blocking** by design (ADR-019): core logic and
/// future local-DB access do not wait on anything, so the app invokes these off
/// the main actor rather than paying for `async`.
#[derive(uniffi::Object)]
pub struct Library {
    store: Mutex<Store>,
}

impl Library {
    /// Runs `f` against the locked [`Store`], recovering from a poisoned mutex.
    ///
    /// A panic in another thread while the lock was held could poison it; since
    /// the corpus is plain data with no broken invariants, we deliberately
    /// recover the guard rather than propagate the poison.
    fn with_store<T>(&self, f: impl FnOnce(&mut Store) -> T) -> T {
        let mut guard = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    /// Convenience over [`Self::with_store`] for the many read/mutate paths that
    /// only touch the content rows.
    fn with_rows<T>(&self, f: impl FnOnce(&mut Vec<Row>) -> T) -> T {
        self.with_store(|store| f(&mut store.rows))
    }

    /// Applies `mutate` to the row with `id`, returning the updated FFI view.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed UUID, or
    /// [`PergamonError::NotFound`] when no row matches.
    fn mutate_item(
        &self,
        id: &str,
        mutate: impl FnOnce(&mut Row),
    ) -> Result<ContentItem, PergamonError> {
        let wanted = Uuid::parse_str(id).map_err(|_| PergamonError::InvalidInput {
            message: format!("not a valid UUID: {id}"),
        })?;
        self.with_rows(|rows| {
            let row = rows.iter_mut().find(|row| row.item.id == wanted).ok_or(
                PergamonError::NotFound {
                    message: format!("no item with id {id}"),
                },
            )?;
            mutate(row);
            Ok(ContentItem::from_row(row))
        })
    }
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
        Arc::new(Self {
            store: Mutex::new(seed()),
        })
    }

    /// Returns every item in triage-`Inbox` status (the primary landing screen).
    #[must_use]
    pub fn inbox(&self) -> Vec<ContentItem> {
        self.items_with_status(Status::Inbox)
    }

    /// Returns all items in the library (the "list" path).
    #[must_use]
    pub fn items(&self) -> Vec<ContentItem> {
        self.with_rows(|rows| rows.iter().map(ContentItem::from_row).collect())
    }

    /// Returns items filtered to a single triage [`Status`].
    #[must_use]
    pub fn items_with_status(&self, status: Status) -> Vec<ContentItem> {
        let core_status: CoreStatus = status.into();
        self.with_rows(|rows| {
            rows.iter()
                .filter(|row| row.item.status == core_status)
                .map(ContentItem::from_row)
                .collect()
        })
    }

    /// Returns the distinct feed/source names present in the corpus, sorted
    /// alphabetically. Drives the inbox's feed filter.
    #[must_use]
    pub fn sources(&self) -> Vec<String> {
        self.with_rows(|rows| {
            let mut names: Vec<String> = rows.iter().filter_map(|row| row.source.clone()).collect();
            names.sort_unstable();
            names.dedup();
            names
        })
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
        self.with_rows(|rows| {
            rows.iter()
                .find(|row| row.item.id == wanted)
                .map(ContentItem::from_row)
                .ok_or(PergamonError::NotFound {
                    message: format!("no item with id {id}"),
                })
        })
    }

    /// Returns items whose title, author, excerpt, URL, or extracted content
    /// contains `query` (case-insensitive). An empty query matches nothing.
    ///
    /// Equivalent to [`Self::search_filtered`] with no facets.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn search(&self, query: String) -> Vec<ContentItem> {
        self.search_filtered(query, SearchFacets::default())
    }

    /// Full-text-ish search AND-combined with optional [`SearchFacets`].
    ///
    /// Text matching is case-insensitive across title, author, excerpt, URL, and
    /// extracted content (mirroring the CLI/web search fields for cross-client
    /// parity). An empty query with no active facets matches nothing; an empty
    /// query with active facets returns every item passing the facets.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn search_filtered(&self, query: String, facets: SearchFacets) -> Vec<ContentItem> {
        let needle = query.trim().to_lowercase();
        let has_facets = facets.content_type.is_some()
            || facets.status.is_some()
            || facets.tag.as_ref().is_some_and(|t| !t.trim().is_empty())
            || facets.source.as_ref().is_some_and(|s| !s.trim().is_empty())
            || facets.since_millis.is_some()
            || facets.before_millis.is_some();
        if needle.is_empty() && !has_facets {
            return Vec::new();
        }
        self.with_rows(|rows| {
            rows.iter()
                .filter(|row| text_matches(row, &needle) && facets_match(row, &facets))
                .map(ContentItem::from_row)
                .collect()
        })
    }

    /// Returns every tag in the registry with its current item count, sorted by
    /// name.
    #[must_use]
    pub fn tags(&self) -> Vec<Tag> {
        self.with_store(|store| {
            let mut tags: Vec<Tag> = store
                .tags
                .iter()
                .map(|tag| {
                    let key = normalize_tag(&tag.name);
                    let item_count = store
                        .rows
                        .iter()
                        .filter(|row| row.tags.iter().any(|t| normalize_tag(t) == key))
                        .count();
                    Tag {
                        id: tag.id.to_string(),
                        name: tag.name.clone(),
                        item_count: u32::try_from(item_count).unwrap_or(u32::MAX),
                    }
                })
                .collect();
            tags.sort_by_key(|a| a.name.to_lowercase());
            tags
        })
    }

    /// Returns every collection with its direct item count and precomputed
    /// nesting depth, ordered so each parent precedes its children (a
    /// depth-first tree order suitable for indented rendering).
    #[must_use]
    pub fn collections(&self) -> Vec<Collection> {
        self.with_store(|store| ordered_collections(store))
    }

    /// Returns items carrying `tag` (case-insensitive). An empty tag matches
    /// nothing.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn items_with_tag(&self, tag: String) -> Vec<ContentItem> {
        let key = normalize_tag(&tag);
        if key.is_empty() {
            return Vec::new();
        }
        self.with_rows(|rows| {
            rows.iter()
                .filter(|row| row.tags.iter().any(|t| normalize_tag(t) == key))
                .map(ContentItem::from_row)
                .collect()
        })
    }

    /// Returns items directly in the collection with `collection_id`.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no collection matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn items_in_collection(
        &self,
        collection_id: String,
    ) -> Result<Vec<ContentItem>, PergamonError> {
        let wanted = parse_uuid(&collection_id)?;
        self.with_store(|store| {
            if !store.collections.iter().any(|c| c.id == wanted) {
                return Err(PergamonError::NotFound {
                    message: format!("no collection with id {collection_id}"),
                });
            }
            Ok(store
                .rows
                .iter()
                .filter(|row| row.collection_ids.contains(&wanted))
                .map(ContentItem::from_row)
                .collect())
        })
    }

    /// Assigns a tag to the item, creating the tag (create-or-reuse by
    /// case-insensitive name) if it does not exist yet. Idempotent. Returns the
    /// updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id or a blank tag name,
    /// or [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_tag(&self, id: String, name: String) -> Result<ContentItem, PergamonError> {
        let wanted = parse_uuid(&id)?;
        let display = name.trim().to_owned();
        if display.is_empty() {
            return Err(PergamonError::InvalidInput {
                message: "tag name must not be blank".to_owned(),
            });
        }
        let key = normalize_tag(&display);
        self.with_store(|store| {
            // Create-or-reuse the registry entry, keeping its first-seen form.
            let canonical =
                if let Some(existing) = store.tags.iter().find(|t| normalize_tag(&t.name) == key) {
                    existing.name.clone()
                } else {
                    store.tags.push(TagRow {
                        id: Uuid::new_v4(),
                        name: display.clone(),
                    });
                    display.clone()
                };
            let row = store
                .rows
                .iter_mut()
                .find(|row| row.item.id == wanted)
                .ok_or(PergamonError::NotFound {
                    message: format!("no item with id {id}"),
                })?;
            if !row.tags.iter().any(|t| normalize_tag(t) == key) {
                row.tags.push(canonical);
                row.item.updated_at = OffsetDateTime::now_utc();
            }
            Ok(ContentItem::from_row(row))
        })
    }

    /// Removes a tag from the item (case-insensitive). Idempotent — removing an
    /// absent tag is a no-op. The tag stays in the registry. Returns the updated
    /// item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn remove_tag(&self, id: String, name: String) -> Result<ContentItem, PergamonError> {
        let key = normalize_tag(&name);
        self.mutate_item(&id, |row| {
            let before = row.tags.len();
            row.tags.retain(|t| normalize_tag(t) != key);
            if row.tags.len() != before {
                row.item.updated_at = OffsetDateTime::now_utc();
            }
        })
    }

    /// Creates a new collection, optionally nested under `parent_id`. Returns the
    /// created collection.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a blank name or a malformed
    /// `parent_id`, or [`PergamonError::NotFound`] when the parent does not
    /// exist.
    #[allow(clippy::needless_pass_by_value)]
    pub fn create_collection(
        &self,
        name: String,
        parent_id: Option<String>,
    ) -> Result<Collection, PergamonError> {
        let display = name.trim().to_owned();
        if display.is_empty() {
            return Err(PergamonError::InvalidInput {
                message: "collection name must not be blank".to_owned(),
            });
        }
        let parent = match parent_id {
            Some(ref raw) => Some(parse_uuid(raw)?),
            None => None,
        };
        self.with_store(|store| {
            if let Some(parent) = parent
                && !store.collections.iter().any(|c| c.id == parent)
            {
                return Err(PergamonError::NotFound {
                    message: format!("no parent collection with id {parent}"),
                });
            }
            let id = Uuid::new_v4();
            store.collections.push(CollectionRow {
                id,
                name: display.clone(),
                parent_id: parent,
            });
            let depth = collection_depth(&store.collections, id);
            Ok(Collection {
                id: id.to_string(),
                name: display,
                parent_id: parent.map(|p| p.to_string()),
                item_count: 0,
                depth,
            })
        })
    }

    /// Adds the item to the collection. Idempotent. Returns the updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when the item or collection does not exist.
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_to_collection(
        &self,
        id: String,
        collection_id: String,
    ) -> Result<ContentItem, PergamonError> {
        let item_id = parse_uuid(&id)?;
        let coll_id = parse_uuid(&collection_id)?;
        self.with_store(|store| {
            if !store.collections.iter().any(|c| c.id == coll_id) {
                return Err(PergamonError::NotFound {
                    message: format!("no collection with id {collection_id}"),
                });
            }
            let row = store
                .rows
                .iter_mut()
                .find(|row| row.item.id == item_id)
                .ok_or(PergamonError::NotFound {
                    message: format!("no item with id {id}"),
                })?;
            if !row.collection_ids.contains(&coll_id) {
                row.collection_ids.push(coll_id);
                row.item.updated_at = OffsetDateTime::now_utc();
            }
            Ok(ContentItem::from_row(row))
        })
    }

    /// Removes the item from the collection. Idempotent — removing an item not in
    /// the collection is a no-op. Returns the updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn remove_from_collection(
        &self,
        id: String,
        collection_id: String,
    ) -> Result<ContentItem, PergamonError> {
        let coll_id = parse_uuid(&collection_id)?;
        self.mutate_item(&id, |row| {
            let before = row.collection_ids.len();
            row.collection_ids.retain(|c| *c != coll_id);
            if row.collection_ids.len() != before {
                row.item.updated_at = OffsetDateTime::now_utc();
            }
        })
    }

    /// Marks the item read, stamping `read_at` with `now` (idempotent — an
    /// already-read item keeps its original timestamp). Returns the updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn mark_read(&self, id: String) -> Result<ContentItem, PergamonError> {
        self.mutate_item(&id, |row| {
            if row.item.read_at.is_none() {
                let now = OffsetDateTime::now_utc();
                row.item.read_at = Some(now);
                row.item.updated_at = now;
            }
        })
    }

    /// Marks the item unread, clearing `read_at`. Returns the updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn mark_unread(&self, id: String) -> Result<ContentItem, PergamonError> {
        self.mutate_item(&id, |row| {
            if row.item.read_at.is_some() {
                row.item.read_at = None;
                row.item.updated_at = OffsetDateTime::now_utc();
            }
        })
    }

    /// Archives the item (status → `Archived`) and marks it read. Returns the
    /// updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn archive(&self, id: String) -> Result<ContentItem, PergamonError> {
        self.mutate_item(&id, |row| {
            let now = OffsetDateTime::now_utc();
            row.item.status = CoreStatus::Archived;
            if row.item.read_at.is_none() {
                row.item.read_at = Some(now);
            }
            row.item.updated_at = now;
        })
    }

    /// Moves the item to the read-later queue (status → `Later`). Returns the
    /// updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn save_for_later(&self, id: String) -> Result<ContentItem, PergamonError> {
        self.mutate_item(&id, |row| {
            row.item.status = CoreStatus::Later;
            row.item.updated_at = OffsetDateTime::now_utc();
        })
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

    #[test]
    fn exposes_content_text_and_source_for_reader_and_filtering() {
        let item = library()
            .item(Uuid::from_u128(1).to_string())
            .expect("seeded");
        assert!(item.content_text.is_some_and(|text| !text.is_empty()));
        assert_eq!(item.source_name.as_deref(), Some("Ink & Switch"));
    }

    #[test]
    fn sources_are_distinct_and_sorted() {
        let sources = library().sources();
        assert_eq!(
            sources,
            vec![
                "Ink & Switch".to_owned(),
                "Memory Weekly".to_owned(),
                "Reader Diaries".to_owned(),
                "Rust Mobile Weekly".to_owned(),
            ]
        );
    }

    #[test]
    fn seeded_inbox_item_starts_unread() {
        let inbox = library().inbox();
        assert!(inbox.iter().all(|item| item.read_at_millis.is_none()));
    }

    #[test]
    fn mark_read_then_unread_toggles_read_state() {
        let lib = library();
        let id = Uuid::from_u128(1).to_string();

        let read = lib.mark_read(id.clone()).expect("seeded");
        assert!(read.read_at_millis.is_some());

        // Idempotent: marking read again keeps the original timestamp.
        let again = lib.mark_read(id.clone()).expect("seeded");
        assert_eq!(again.read_at_millis, read.read_at_millis);

        let unread = lib.mark_unread(id).expect("seeded");
        assert!(unread.read_at_millis.is_none());
    }

    #[test]
    fn archive_sets_status_and_marks_read() {
        let lib = library();
        let id = Uuid::from_u128(1).to_string();
        let archived = lib.archive(id).expect("seeded");
        assert_eq!(archived.status, Status::Archived);
        assert!(archived.read_at_millis.is_some());
        // The change is observable through subsequent reads.
        assert_eq!(lib.items_with_status(Status::Archived).len(), 2);
        assert!(lib.inbox().is_empty());
    }

    #[test]
    fn save_for_later_moves_item_to_later() {
        let lib = library();
        let id = Uuid::from_u128(1).to_string();
        let saved = lib.save_for_later(id).expect("seeded");
        assert_eq!(saved.status, Status::Later);
        assert_eq!(lib.items_with_status(Status::Later).len(), 2);
    }

    #[test]
    fn mutations_reject_malformed_and_unknown_ids() {
        let lib = library();
        assert!(matches!(
            lib.mark_read("not-a-uuid".to_owned()),
            Err(PergamonError::InvalidInput { .. })
        ));
        assert!(matches!(
            lib.archive(Uuid::from_u128(999).to_string()),
            Err(PergamonError::NotFound { .. })
        ));
    }

    // ---- Organization: tags & collections -----------------------------------

    fn id(n: u128) -> String {
        Uuid::from_u128(n).to_string()
    }

    #[test]
    fn seeded_item_exposes_tags_and_collections() {
        let item = library().item(id(1)).expect("seeded");
        assert_eq!(
            item.tags,
            vec!["local-first".to_owned(), "reading".to_owned()]
        );
        assert_eq!(item.collection_ids, vec![id(101)]);
    }

    #[test]
    fn tags_are_sorted_with_item_counts() {
        let tags = library().tags();
        let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["ios", "local-first", "memory", "reading", "rust"]
        );
        let reading = tags
            .iter()
            .find(|t| t.name == "reading")
            .expect("reading tag");
        assert_eq!(reading.item_count, 3);
        let rust = tags.iter().find(|t| t.name == "rust").expect("rust tag");
        assert_eq!(rust.item_count, 1);
    }

    #[test]
    fn collections_are_tree_ordered_with_depth_and_counts() {
        let collections = library().collections();
        let shape: Vec<(&str, u32, u32)> = collections
            .iter()
            .map(|c| (c.name.as_str(), c.depth, c.item_count))
            .collect();
        // Deep Dives nests under Reading List; Tech is a second root.
        assert_eq!(
            shape,
            vec![("Reading List", 0, 2), ("Deep Dives", 1, 1), ("Tech", 0, 1),]
        );
    }

    #[test]
    fn items_with_tag_is_case_insensitive() {
        let lib = library();
        assert_eq!(lib.items_with_tag("READING".to_owned()).len(), 3);
        assert_eq!(lib.items_with_tag("rust".to_owned()).len(), 1);
        assert!(lib.items_with_tag("  ".to_owned()).is_empty());
        assert!(lib.items_with_tag("no-such-tag".to_owned()).is_empty());
    }

    #[test]
    fn items_in_collection_lists_members_and_rejects_unknown() {
        let lib = library();
        let reading = lib.items_in_collection(id(101)).expect("seeded collection");
        assert_eq!(reading.len(), 2);
        match lib.items_in_collection(id(999)) {
            Err(PergamonError::NotFound { .. }) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
        match lib.items_in_collection("not-a-uuid".to_owned()) {
            Err(PergamonError::InvalidInput { .. }) => {}
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn add_tag_creates_and_is_idempotent_and_reuses_by_case() {
        let lib = library();
        // Item 4 starts untagged.
        let tagged = lib.add_tag(id(4), "Focus".to_owned()).expect("seeded");
        assert_eq!(tagged.tags, vec!["Focus".to_owned()]);
        // New tag registered.
        assert!(lib.tags().iter().any(|t| t.name == "Focus"));

        // Idempotent, case-insensitive: no duplicate.
        let again = lib.add_tag(id(4), "focus".to_owned()).expect("seeded");
        assert_eq!(again.tags, vec!["Focus".to_owned()]);

        // Reuses an existing registry entry rather than adding a second one.
        let before = lib.tags().len();
        lib.add_tag(id(5), "READING".to_owned()).expect("seeded");
        assert_eq!(lib.tags().len(), before);
    }

    #[test]
    fn add_tag_rejects_blank_and_unknown_item() {
        let lib = library();
        assert!(matches!(
            lib.add_tag(id(1), "   ".to_owned()),
            Err(PergamonError::InvalidInput { .. })
        ));
        assert!(matches!(
            lib.add_tag(id(999), "x".to_owned()),
            Err(PergamonError::NotFound { .. })
        ));
    }

    #[test]
    fn remove_tag_is_idempotent() {
        let lib = library();
        let removed = lib.remove_tag(id(1), "READING".to_owned()).expect("seeded");
        assert_eq!(removed.tags, vec!["local-first".to_owned()]);
        // Removing again is a no-op.
        let again = lib.remove_tag(id(1), "reading".to_owned()).expect("seeded");
        assert_eq!(again.tags, vec!["local-first".to_owned()]);
    }

    #[test]
    fn create_collection_nests_and_validates() {
        let lib = library();
        let created = lib
            .create_collection("Later Reads".to_owned(), Some(id(101)))
            .expect("valid parent");
        assert_eq!(created.name, "Later Reads");
        assert_eq!(created.parent_id, Some(id(101)));
        assert_eq!(created.depth, 1);
        assert_eq!(created.item_count, 0);
        assert!(lib.collections().iter().any(|c| c.id == created.id));

        assert!(matches!(
            lib.create_collection("  ".to_owned(), None),
            Err(PergamonError::InvalidInput { .. })
        ));
        assert!(matches!(
            lib.create_collection("Orphan".to_owned(), Some(id(999))),
            Err(PergamonError::NotFound { .. })
        ));
    }

    #[test]
    fn add_and_remove_from_collection_are_idempotent() {
        let lib = library();
        // Item 4 starts in no collection.
        let added = lib.add_to_collection(id(4), id(103)).expect("seeded");
        assert_eq!(added.collection_ids, vec![id(103)]);
        assert_eq!(lib.items_in_collection(id(103)).expect("tech").len(), 2);

        // Idempotent add.
        let again = lib.add_to_collection(id(4), id(103)).expect("seeded");
        assert_eq!(again.collection_ids, vec![id(103)]);

        let removed = lib.remove_from_collection(id(4), id(103)).expect("seeded");
        assert!(removed.collection_ids.is_empty());
        // Removing again is a no-op.
        let noop = lib.remove_from_collection(id(4), id(103)).expect("seeded");
        assert!(noop.collection_ids.is_empty());
    }

    #[test]
    fn add_to_collection_rejects_unknown_collection() {
        let lib = library();
        assert!(matches!(
            lib.add_to_collection(id(1), id(999)),
            Err(PergamonError::NotFound { .. })
        ));
    }

    // ---- Faceted search ------------------------------------------------------

    #[test]
    fn search_filtered_matches_content_text() {
        // The seeded bodies are runs of "word"; a plain search hits everything.
        let lib = library();
        assert_eq!(lib.search("word".to_owned()).len(), 5);
    }

    #[test]
    fn search_filtered_and_combines_facets() {
        let lib = library();

        // Tag facet.
        let facets = SearchFacets {
            tag: Some("reading".to_owned()),
            ..Default::default()
        };
        assert_eq!(lib.search_filtered(String::new(), facets).len(), 3);

        // Status facet.
        let facets = SearchFacets {
            status: Some(Status::Later),
            ..Default::default()
        };
        assert_eq!(lib.search_filtered(String::new(), facets).len(), 1);

        // Content type facet.
        let facets = SearchFacets {
            content_type: Some(ContentType::Article),
            ..Default::default()
        };
        assert_eq!(lib.search_filtered(String::new(), facets).len(), 2);

        // Source facet.
        let facets = SearchFacets {
            source: Some("Ink & Switch".to_owned()),
            ..Default::default()
        };
        assert_eq!(lib.search_filtered(String::new(), facets).len(), 1);

        // Text + facet AND: "word" matches all, tag narrows to reading items.
        let facets = SearchFacets {
            tag: Some("rust".to_owned()),
            ..Default::default()
        };
        let hits = lib.search_filtered("word".to_owned(), facets);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].tags.iter().any(|t| t == "rust"));
    }

    #[test]
    fn search_filtered_applies_date_range() {
        let lib = library();
        // Seeded publications span 2020-01-01 .. 2024-01-01 (epoch millis).
        let facets = SearchFacets {
            since_millis: Some(1_640_995_200_000), // 2022-01-01
            ..Default::default()
        };
        // Items 3 (2022), 4 (2023), 5 (2024).
        assert_eq!(lib.search_filtered(String::new(), facets).len(), 3);

        let facets = SearchFacets {
            before_millis: Some(1_609_459_200_000), // 2021-01-01 (exclusive)
            ..Default::default()
        };
        // Only item 1 (2020).
        assert_eq!(lib.search_filtered(String::new(), facets).len(), 1);
    }

    #[test]
    fn search_filtered_empty_query_no_facets_matches_nothing() {
        assert!(
            library()
                .search_filtered(String::new(), SearchFacets::default())
                .is_empty()
        );
    }
}
