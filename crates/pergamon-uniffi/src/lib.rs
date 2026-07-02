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
use pergamon_core::fsrs::{CardState, MemoryState, Parameters, Rating, Scheduler};
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

/// The grade a user assigns a review card, mirroring
/// `pergamon_core::fsrs::Rating`. Drives the FSRS scheduler: `Again` means the
/// material was forgotten (reschedule soon), `Easy` means it was recalled
/// effortlessly (longest interval).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReviewGrade {
    /// Forgot the material — schedule again soon.
    Again,
    /// Recalled with significant difficulty.
    Hard,
    /// Recalled correctly.
    Good,
    /// Recalled effortlessly.
    Easy,
}

impl From<ReviewGrade> for Rating {
    fn from(value: ReviewGrade) -> Self {
        match value {
            ReviewGrade::Again => Self::Again,
            ReviewGrade::Hard => Self::Hard,
            ReviewGrade::Good => Self::Good,
            ReviewGrade::Easy => Self::Easy,
        }
    }
}

/// The lifecycle state of a review card, mirroring
/// `pergamon_core::fsrs::CardState`. Surfaced so the app can badge cards
/// (new vs. learning vs. review vs. relearning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReviewState {
    /// Card has never been reviewed.
    New,
    /// Card is in the initial learning phase.
    Learning,
    /// Card is in the long-term review phase.
    Review,
    /// Card was forgotten and is being relearned.
    Relearning,
}

impl From<CardState> for ReviewState {
    fn from(value: CardState) -> Self {
        match value {
            CardState::New => Self::New,
            CardState::Learning => Self::Learning,
            CardState::Review => Self::Review,
            CardState::Relearning => Self::Relearning,
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

/// An FFI-friendly view of a user highlight captured from an item, mirroring
/// `pergamon_core::model::HighlightMeta` plus the review-card link the app
/// needs.
///
/// A highlight is a quote pulled from a source item, optionally annotated with a
/// note. `id` is a UUID string and `created_at_millis` is Unix epoch
/// milliseconds. `has_review_card` reflects whether a spaced-repetition card was
/// created for this highlight (the facade creates one automatically on capture).
#[derive(Debug, Clone, uniffi::Record)]
pub struct Highlight {
    /// Stable UUID, serialized as a string.
    pub id: String,
    /// UUID string of the content item this highlight was captured from.
    pub item_id: String,
    /// The highlighted quote text.
    pub quote_text: String,
    /// User note attached to the highlight, if any.
    pub note: Option<String>,
    /// Title of the source item, denormalized for display.
    pub source_title: String,
    /// When the highlight was captured, as Unix epoch milliseconds.
    pub created_at_millis: i64,
    /// Whether a spaced-repetition review card exists for this highlight.
    pub has_review_card: bool,
}

/// An FFI-friendly view of a spaced-repetition review card, joined with its
/// backing highlight for display in the review queue.
///
/// Mirrors the scheduling fields of `pergamon_core::model::ReviewCard` the app
/// needs, plus the highlight's quote/note/source so a queue card renders without
/// a second round-trip. `due_at_millis` and `last_reviewed_at_millis` are Unix
/// epoch milliseconds.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ReviewCardView {
    /// Stable UUID of the review card, serialized as a string.
    pub card_id: String,
    /// UUID string of the highlight this card reviews.
    pub highlight_id: String,
    /// UUID string of the content item the highlight came from.
    pub item_id: String,
    /// The highlighted quote text (the review prompt).
    pub quote_text: String,
    /// User note attached to the highlight, revealed as the answer, if any.
    pub note: Option<String>,
    /// Title of the source item, denormalized for display.
    pub source_title: String,
    /// Current lifecycle state of the card.
    pub state: ReviewState,
    /// When the card is next due, as Unix epoch milliseconds.
    pub due_at_millis: i64,
    /// Total number of reviews performed on this card.
    pub review_count: u32,
    /// When the card was last reviewed, as Unix epoch milliseconds. `None` if
    /// never reviewed.
    pub last_reviewed_at_millis: Option<i64>,
}

/// Aggregate review counters for surfacing the due-count and queue health,
/// mirroring the fields of `pergamon_core::model::ReviewStats` the app shows.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ReviewSummary {
    /// Number of cards currently due for review (`due_at <= now`).
    pub due_count: u32,
    /// Total number of review cards across the library.
    pub total_cards: u32,
    /// Number of cards that have never been reviewed.
    pub new_count: u32,
    /// Number of reviews completed today (UTC).
    pub reviews_today: u32,
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

/// An internal highlight row: a quote captured from a content item plus an
/// optional note. Mirrors the fields of `pergamon_core::model::HighlightMeta`
/// the facade exposes, with its own identity and capture timestamp.
#[derive(Debug, Clone)]
struct HighlightRow {
    id: Uuid,
    item_id: Uuid,
    quote_text: String,
    note: Option<String>,
    created_at: OffsetDateTime,
}

/// An internal spaced-repetition card row, mirroring the scheduling fields of
/// `pergamon_core::model::ReviewCard`. One card backs one highlight. Scheduling
/// is driven by `pergamon_core::fsrs::Scheduler`, the same engine the CLI uses,
/// so review state stays consistent across clients on the same library.
#[derive(Debug, Clone)]
struct ReviewCardRow {
    id: Uuid,
    highlight_id: Uuid,
    state: CardState,
    stability: Option<f64>,
    difficulty: Option<f64>,
    due_at: OffsetDateTime,
    last_reviewed_at: Option<OffsetDateTime>,
    review_count: i32,
    lapse_count: i32,
    scheduled_days: Option<f64>,
}

/// An internal review-log row recording a single grade event, mirroring
/// `pergamon_core::model::ReviewLog`. Grade buttons append one of these so the
/// facade can report reviews-per-day and keep a per-card history.
#[derive(Debug, Clone)]
struct ReviewLogRow {
    card_id: Uuid,
    reviewed_at: OffsetDateTime,
}

/// The facade's in-memory state: content rows plus the tag and collection
/// registries, and the highlight / review-card / review-log tables. Guarded by
/// a single `Mutex` so multi-entity mutations (adding a tag both registers it
/// and stamps the row; capturing a highlight both records it and creates a card)
/// stay consistent without lock ordering concerns.
#[derive(Debug, Clone)]
struct Store {
    rows: Vec<Row>,
    tags: Vec<TagRow>,
    collections: Vec<CollectionRow>,
    highlights: Vec<HighlightRow>,
    review_cards: Vec<ReviewCardRow>,
    review_logs: Vec<ReviewLogRow>,
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

/// Normalizes an optional note: trims whitespace and collapses a blank note to
/// `None`, so an empty text field clears the note rather than storing "".
fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty())
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

impl Store {
    /// The display title of the content item a highlight/card belongs to,
    /// denormalized into the FFI views so the app renders a queue card without a
    /// second lookup. Falls back to a placeholder for an orphaned reference.
    fn source_title(&self, item_id: Uuid) -> String {
        self.rows
            .iter()
            .find(|row| row.item.id == item_id)
            .map_or_else(|| "Unknown source".to_owned(), |row| row.item.title.clone())
    }

    /// Builds the FFI [`Highlight`] view from an internal row, folding in the
    /// source title and whether a review card exists.
    fn highlight_view(&self, hl: &HighlightRow) -> Highlight {
        Highlight {
            id: hl.id.to_string(),
            item_id: hl.item_id.to_string(),
            quote_text: hl.quote_text.clone(),
            note: hl.note.clone(),
            source_title: self.source_title(hl.item_id),
            created_at_millis: millis(hl.created_at),
            has_review_card: self.review_cards.iter().any(|c| c.highlight_id == hl.id),
        }
    }

    /// Builds the FFI [`ReviewCardView`] by joining a card with its backing
    /// highlight. Returns `None` if the highlight has gone (a dangling card),
    /// which callers filter out.
    fn card_view(&self, card: &ReviewCardRow) -> Option<ReviewCardView> {
        let hl = self.highlights.iter().find(|h| h.id == card.highlight_id)?;
        Some(ReviewCardView {
            card_id: card.id.to_string(),
            highlight_id: hl.id.to_string(),
            item_id: hl.item_id.to_string(),
            quote_text: hl.quote_text.clone(),
            note: hl.note.clone(),
            source_title: self.source_title(hl.item_id),
            state: card.state.into(),
            due_at_millis: millis(card.due_at),
            review_count: u32::try_from(card.review_count).unwrap_or(0),
            last_reviewed_at_millis: card.last_reviewed_at.map(millis),
        })
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

    // Seed a couple of highlights (with fixed ids) so the reader shows captured
    // annotations and the review queue / due-count are populated on first
    // launch. Each highlight gets a New card due at seed time (well in the past),
    // so both are due immediately.
    let due = at(1_577_836_800); // 2020-01-01, comfortably in the past
    let hl_local = Uuid::from_u128(301);
    let hl_fsrs = Uuid::from_u128(302);
    let highlights = vec![
        HighlightRow {
            id: hl_local,
            item_id: Uuid::from_u128(1),
            quote_text: "You own your data, in spite of the cloud.".to_owned(),
            note: Some("The core promise of local-first software.".to_owned()),
            created_at: due,
        },
        HighlightRow {
            id: hl_fsrs,
            item_id: Uuid::from_u128(2),
            quote_text: "FSRS models memory as stability and difficulty.".to_owned(),
            note: None,
            created_at: due,
        },
    ];
    let review_cards = vec![
        ReviewCardRow {
            id: Uuid::from_u128(401),
            highlight_id: hl_local,
            state: CardState::New,
            stability: None,
            difficulty: None,
            due_at: due,
            last_reviewed_at: None,
            review_count: 0,
            lapse_count: 0,
            scheduled_days: None,
        },
        ReviewCardRow {
            id: Uuid::from_u128(402),
            highlight_id: hl_fsrs,
            state: CardState::New,
            stability: None,
            difficulty: None,
            due_at: due,
            last_reviewed_at: None,
            review_count: 0,
            lapse_count: 0,
            scheduled_days: None,
        },
    ];

    Store {
        rows,
        tags,
        collections,
        highlights,
        review_cards,
        review_logs: Vec::new(),
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

    // ---- Highlights & spaced-repetition review -------------------------------

    /// Lists the highlights captured from a content item, oldest first.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn highlights(&self, item_id: String) -> Result<Vec<Highlight>, PergamonError> {
        let wanted = parse_uuid(&item_id)?;
        self.with_store(|store| {
            if !store.rows.iter().any(|row| row.item.id == wanted) {
                return Err(PergamonError::NotFound {
                    message: format!("no item with id {item_id}"),
                });
            }
            let mut list: Vec<&HighlightRow> = store
                .highlights
                .iter()
                .filter(|h| h.item_id == wanted)
                .collect();
            list.sort_by_key(|h| h.created_at);
            Ok(list.into_iter().map(|h| store.highlight_view(h)).collect())
        })
    }

    /// Captures a highlight from a content item and creates a spaced-repetition
    /// review card for it (New, due immediately) so it enters the review queue.
    ///
    /// The `quote_text` is trimmed and must be non-empty; `note` is trimmed and
    /// treated as absent when blank. Returns the newly captured highlight.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id or blank quote, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_highlight(
        &self,
        item_id: String,
        quote_text: String,
        note: Option<String>,
    ) -> Result<Highlight, PergamonError> {
        let wanted = parse_uuid(&item_id)?;
        let quote = quote_text.trim().to_owned();
        if quote.is_empty() {
            return Err(PergamonError::InvalidInput {
                message: "highlight quote must not be empty".to_owned(),
            });
        }
        let note = clean_note(note);
        self.with_store(|store| {
            if !store.rows.iter().any(|row| row.item.id == wanted) {
                return Err(PergamonError::NotFound {
                    message: format!("no item with id {item_id}"),
                });
            }
            let now = OffsetDateTime::now_utc();
            let highlight_id = Uuid::new_v4();
            let row = HighlightRow {
                id: highlight_id,
                item_id: wanted,
                quote_text: quote,
                note,
                created_at: now,
            };
            // Build the FFI view up front from known data (the highlight always
            // gets a card, below), then move the row into the store.
            let view = Highlight {
                id: highlight_id.to_string(),
                item_id: wanted.to_string(),
                quote_text: row.quote_text.clone(),
                note: row.note.clone(),
                source_title: store.source_title(wanted),
                created_at_millis: millis(now),
                has_review_card: true,
            };
            store.highlights.push(row);
            // Auto-create a New card, due now, so the highlight lands in the
            // queue and bumps the due-count immediately.
            store.review_cards.push(ReviewCardRow {
                id: Uuid::new_v4(),
                highlight_id,
                state: CardState::New,
                stability: None,
                difficulty: None,
                due_at: now,
                last_reviewed_at: None,
                review_count: 0,
                lapse_count: 0,
                scheduled_days: None,
            });
            Ok(view)
        })
    }

    /// Sets or clears the note on a highlight. A blank note clears it. Returns
    /// the updated highlight.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no highlight matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_highlight_note(
        &self,
        highlight_id: String,
        note: Option<String>,
    ) -> Result<Highlight, PergamonError> {
        let wanted = parse_uuid(&highlight_id)?;
        let note = clean_note(note);
        self.with_store(|store| {
            let Some(hl) = store.highlights.iter_mut().find(|h| h.id == wanted) else {
                return Err(PergamonError::NotFound {
                    message: format!("no highlight with id {highlight_id}"),
                });
            };
            hl.note = note;
            let updated = hl.clone();
            Ok(store.highlight_view(&updated))
        })
    }

    /// Deletes a highlight along with its review card and logs. Idempotent: a
    /// no-op if the highlight is already gone.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id.
    #[allow(clippy::needless_pass_by_value)]
    pub fn delete_highlight(&self, highlight_id: String) -> Result<(), PergamonError> {
        let wanted = parse_uuid(&highlight_id)?;
        self.with_store(|store| {
            let card_ids: Vec<Uuid> = store
                .review_cards
                .iter()
                .filter(|c| c.highlight_id == wanted)
                .map(|c| c.id)
                .collect();
            store.highlights.retain(|h| h.id != wanted);
            store.review_cards.retain(|c| c.highlight_id != wanted);
            store.review_logs.retain(|l| !card_ids.contains(&l.card_id));
        });
        Ok(())
    }

    /// Returns the review cards currently due (`due_at <= now`), soonest-due
    /// first. This is the review queue the app grades through.
    #[must_use]
    pub fn due_cards(&self) -> Vec<ReviewCardView> {
        let now = OffsetDateTime::now_utc();
        self.with_store(|store| {
            let mut due: Vec<&ReviewCardRow> = store
                .review_cards
                .iter()
                .filter(|c| c.due_at <= now)
                .collect();
            due.sort_by_key(|c| c.due_at);
            due.into_iter().filter_map(|c| store.card_view(c)).collect()
        })
    }

    /// Grades a review card, advancing its FSRS schedule and appending a review
    /// log. Uses the same engine and default parameters as the CLI, so review
    /// state stays consistent across clients on one library. Returns the updated
    /// card view (with its new due date and state).
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no card matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn grade_card(
        &self,
        card_id: String,
        grade: ReviewGrade,
    ) -> Result<ReviewCardView, PergamonError> {
        let wanted = parse_uuid(&card_id)?;
        let rating: Rating = grade.into();
        self.with_store(|store| {
            let Some(card) = store.review_cards.iter().find(|c| c.id == wanted) else {
                return Err(PergamonError::NotFound {
                    message: format!("no review card with id {card_id}"),
                });
            };

            let now = OffsetDateTime::now_utc();
            let elapsed_days = card
                .last_reviewed_at
                .map_or(0.0, |last| (now - last).as_seconds_f64() / 86_400.0);
            let memory = match (card.stability, card.difficulty) {
                (Some(stability), Some(difficulty)) => Some(MemoryState {
                    stability,
                    difficulty,
                }),
                _ => None,
            };

            let scheduler = Scheduler::new(&Parameters::default());
            let output = scheduler.schedule(card.state, memory, elapsed_days, rating);
            let due_at = now + time::Duration::seconds_f64(output.scheduled_days * 86_400.0);

            let Some(card) = store.review_cards.iter_mut().find(|c| c.id == wanted) else {
                return Err(PergamonError::NotFound {
                    message: format!("no review card with id {card_id}"),
                });
            };
            card.state = output.next_state;
            card.stability = Some(output.memory.stability);
            card.difficulty = Some(output.memory.difficulty);
            card.due_at = due_at;
            card.last_reviewed_at = Some(now);
            card.review_count += 1;
            if rating == Rating::Again {
                card.lapse_count += 1;
            }
            card.scheduled_days = Some(output.scheduled_days);
            let updated = card.clone();

            store.review_logs.push(ReviewLogRow {
                card_id: wanted,
                reviewed_at: now,
            });

            store.card_view(&updated).ok_or(PergamonError::NotFound {
                message: format!("highlight for card {card_id} is gone"),
            })
        })
    }

    /// Returns aggregate review counters for surfacing the due-count badge and
    /// queue health.
    #[must_use]
    pub fn review_summary(&self) -> ReviewSummary {
        let now = OffsetDateTime::now_utc();
        self.with_store(|store| {
            let due_count = store
                .review_cards
                .iter()
                .filter(|c| c.due_at <= now)
                .count();
            let new_count = store
                .review_cards
                .iter()
                .filter(|c| c.state == CardState::New)
                .count();
            let reviews_today = store
                .review_logs
                .iter()
                .filter(|l| l.reviewed_at.date() == now.date())
                .count();
            ReviewSummary {
                due_count: u32::try_from(due_count).unwrap_or(u32::MAX),
                total_cards: u32::try_from(store.review_cards.len()).unwrap_or(u32::MAX),
                new_count: u32::try_from(new_count).unwrap_or(u32::MAX),
                reviews_today: u32::try_from(reviews_today).unwrap_or(u32::MAX),
            }
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

    // ---- Highlights & spaced-repetition review -------------------------------

    #[test]
    fn seeded_highlights_and_due_cards_are_present() {
        let lib = library();
        // Item 1 has one seeded highlight, with a note.
        let highlights = lib.highlights(id(1)).expect("seeded item");
        assert_eq!(highlights.len(), 1);
        assert!(highlights[0].note.is_some());
        assert!(highlights[0].has_review_card);
        assert_eq!(
            highlights[0].source_title,
            "Local-first software: you own your data"
        );

        // Two seeded cards, both due now.
        assert_eq!(lib.due_cards().len(), 2);
        let summary = lib.review_summary();
        assert_eq!(summary.due_count, 2);
        assert_eq!(summary.total_cards, 2);
        assert_eq!(summary.new_count, 2);
        assert_eq!(summary.reviews_today, 0);
    }

    #[test]
    fn highlights_validates_item_id() {
        let lib = library();
        assert!(matches!(
            lib.highlights("not-a-uuid".to_owned()),
            Err(PergamonError::InvalidInput { .. })
        ));
        assert!(matches!(
            lib.highlights(id(999)),
            Err(PergamonError::NotFound { .. })
        ));
    }

    #[test]
    fn add_highlight_captures_and_auto_creates_a_due_card() {
        let lib = library();
        let before = lib.due_cards().len();

        let hl = lib
            .add_highlight(
                id(3),
                "  A captured quote  ".to_owned(),
                Some("my note".to_owned()),
            )
            .expect("valid item");
        // Quote is trimmed; note preserved; card created.
        assert_eq!(hl.quote_text, "A captured quote");
        assert_eq!(hl.note.as_deref(), Some("my note"));
        assert!(hl.has_review_card);
        assert_eq!(hl.item_id, id(3));

        // The new highlight shows up for its item and the queue grew by one.
        assert_eq!(lib.highlights(id(3)).expect("item").len(), 1);
        assert_eq!(lib.due_cards().len(), before + 1);
        assert_eq!(
            lib.review_summary().due_count,
            u32::try_from(before + 1).unwrap()
        );
    }

    #[test]
    fn add_highlight_rejects_blank_quote_and_unknown_item() {
        let lib = library();
        assert!(matches!(
            lib.add_highlight(id(1), "   ".to_owned(), None),
            Err(PergamonError::InvalidInput { .. })
        ));
        assert!(matches!(
            lib.add_highlight(id(999), "quote".to_owned(), None),
            Err(PergamonError::NotFound { .. })
        ));
    }

    #[test]
    fn add_highlight_treats_blank_note_as_absent() {
        let lib = library();
        let hl = lib
            .add_highlight(id(3), "quote".to_owned(), Some("   ".to_owned()))
            .expect("valid item");
        assert!(hl.note.is_none());
    }

    #[test]
    fn set_highlight_note_updates_and_clears() {
        let lib = library();
        let hl = lib
            .add_highlight(id(3), "quote".to_owned(), None)
            .expect("valid");

        let noted = lib
            .set_highlight_note(hl.id.clone(), Some("added later".to_owned()))
            .expect("exists");
        assert_eq!(noted.note.as_deref(), Some("added later"));

        // Blank note clears it.
        let cleared = lib
            .set_highlight_note(hl.id.clone(), Some("  ".to_owned()))
            .expect("exists");
        assert!(cleared.note.is_none());

        assert!(matches!(
            lib.set_highlight_note(id(999), None),
            Err(PergamonError::NotFound { .. })
        ));
    }

    #[test]
    fn delete_highlight_removes_highlight_and_card() {
        let lib = library();
        let hl = lib
            .add_highlight(id(3), "quote".to_owned(), None)
            .expect("valid");
        let due_before = lib.due_cards().len();

        lib.delete_highlight(hl.id.clone()).expect("valid id");
        assert!(lib.highlights(id(3)).expect("item").is_empty());
        assert_eq!(lib.due_cards().len(), due_before - 1);

        // Idempotent.
        lib.delete_highlight(hl.id).expect("no-op");
        // Malformed id still validated.
        assert!(matches!(
            lib.delete_highlight("not-a-uuid".to_owned()),
            Err(PergamonError::InvalidInput { .. })
        ));
    }

    #[test]
    fn grade_card_advances_schedule_and_writes_a_log() {
        let lib = library();
        let card = lib.due_cards().into_iter().next().expect("seeded card");
        assert_eq!(card.review_count, 0);
        assert_eq!(card.state, ReviewState::New);

        let graded = lib
            .grade_card(card.card_id.clone(), ReviewGrade::Good)
            .expect("valid card");

        // A "Good" grade on a new card moves it to Review and schedules it out,
        // so it is no longer due — the queue shrinks and a review is logged.
        assert_eq!(graded.review_count, 1);
        assert_eq!(graded.state, ReviewState::Review);
        assert!(graded.due_at_millis > card.due_at_millis);
        assert!(graded.last_reviewed_at_millis.is_some());

        assert_eq!(lib.due_cards().len(), 1);
        assert_eq!(lib.review_summary().reviews_today, 1);
        assert_eq!(lib.review_summary().due_count, 1);
    }

    #[test]
    fn grade_card_again_keeps_card_due_soon() {
        let lib = library();
        let card = lib.due_cards().into_iter().next().expect("seeded card");
        let graded = lib
            .grade_card(card.card_id, ReviewGrade::Again)
            .expect("valid card");
        // "Again" schedules a short interval; the card is still learning.
        assert_eq!(graded.state, ReviewState::Learning);
        assert_eq!(graded.review_count, 1);
    }

    #[test]
    fn grade_card_validates_id() {
        let lib = library();
        assert!(matches!(
            lib.grade_card("not-a-uuid".to_owned(), ReviewGrade::Good),
            Err(PergamonError::InvalidInput { .. })
        ));
        assert!(matches!(
            lib.grade_card(id(999), ReviewGrade::Good),
            Err(PergamonError::NotFound { .. })
        ));
    }
}
