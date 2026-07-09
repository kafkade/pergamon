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
//!   `archive`, `save_for_later`, ...). Backed by the on-device SQLite store
//!   (`pergamon-storage`) so reads and mutations persist across launches
//!   (#118 / ADR-020). Open the persistent library with [`Library::open`];
//!   [`Library::new`] keeps an in-memory seeded corpus for tests and previews.
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

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use pergamon_core::content_type::ContentType as CoreContentType;
use pergamon_core::error::CoreError;
use pergamon_core::fsrs::{CardState, MemoryState, Parameters, Rating, Scheduler};
use pergamon_core::model::{
    BookmarkMeta as CoreBookmarkMeta, Collection as CoreCollection, ContentItem as CoreContentItem,
    Feed as CoreFeed, FeedItemMeta as CoreFeedItemMeta, HighlightMeta as CoreHighlightMeta,
    ReviewCard as CoreReviewCard, ReviewLog as CoreReviewLog,
};
use pergamon_core::reading_time::reading_time_from_text;
use pergamon_core::status::DocumentStatus as CoreStatus;

use pergamon_storage::backup;
use pergamon_storage::{BackupStats, Database, StorageError};

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

/// What a staged share-sheet capture carries, mirroring ADR-021's
/// `content_kind` discriminator. It is a *hint* for finalization, not a trust
/// boundary: the presence of `url` / `selected_text` on [`ShareCapture`] is what
/// actually drives ingestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ShareContentKind {
    /// A bare URL shared from Safari or another app.
    Url,
    /// A URL shared together with a text selection from the page.
    UrlWithSelection,
    /// A standalone text selection with no source URL.
    Text,
}

/// One staged capture handed off from the iOS share extension, per **ADR-021**.
///
/// The extension writes these as atomic JSON drop files to the shared App Group
/// container; the main app decodes each one and passes it to
/// [`Library::ingest_share_capture`], which runs the *same* ingestion pipeline
/// the CLI `save` command uses (canonicalize → dedupe → create/enrich → attach
/// highlight). The extension itself does no network, extraction, or database
/// work — it only serializes this record.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ShareCapture {
    /// Stable UUID (string) for this capture; also the drop-file name and the
    /// idempotency key for text-only captures. Reprocessing the same
    /// `capture_id` must converge on the same item rather than duplicate it.
    pub capture_id: String,
    /// When the capture happened, as Unix epoch milliseconds (ADR-019 time
    /// mapping). The app drains oldest-first by this value.
    pub captured_at_millis: i64,
    /// The kind of capture (a finalization hint; see [`ShareContentKind`]).
    pub content_kind: ShareContentKind,
    /// The raw shared URL, *not* yet canonicalized. Present for `Url` /
    /// `UrlWithSelection`.
    pub url: Option<String>,
    /// The shared / selected text. Present for `UrlWithSelection` / `Text`.
    pub selected_text: Option<String>,
    /// Title supplied by the share sheet (e.g. Safari's page title), stored
    /// without a fetch.
    pub page_title: Option<String>,
    /// Best-effort originating bundle id, kept for provenance.
    pub source_app: Option<String>,
}

/// The result of finalizing one [`ShareCapture`], so the app can report what
/// happened and refresh the right surfaces.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ShareIngestOutcome {
    /// UUID string of the URL-backed content item created or reused, if the
    /// capture carried a URL. `None` for text-only captures.
    pub item_id: Option<String>,
    /// UUID string of the highlight created from the selection, if any.
    pub highlight_id: Option<String>,
    /// `true` when the capture matched an existing item/highlight (by canonical
    /// URL, or by `capture_id` for text-only) and so added nothing new.
    pub deduped: bool,
}

fn millis(dt: OffsetDateTime) -> i64 {
    // nanoseconds since the Unix epoch, narrowed to milliseconds. Any realistic
    // calendar date fits comfortably in i64 milliseconds.
    #[allow(clippy::cast_possible_truncation)]
    let ms = (dt.unix_timestamp_nanos() / 1_000_000) as i64;
    ms
}

/// Inverse of [`millis`]: builds a UTC timestamp from Unix epoch milliseconds,
/// falling back to the Unix epoch for an out-of-range value rather than failing
/// (a staged capture's `captured_at` is provenance, not correctness-critical).
fn from_millis(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `BookmarkMeta.saved_from` provenance stamp for items ingested from the iOS
/// share sheet (ADR-021). Kept distinct from CLI (`browser`) and web (`web`).
const SHARE_SHEET_SOURCE: &str = "share-sheet";

/// Builds the `BookmarkMeta` for a share-sheet capture: records the raw shared
/// URL as `original_url` and stamps `saved_from = "share-sheet"` for provenance.
/// Richer enrichment (favicon, site name, description) is filled by later
/// extraction, so it is left `None` here.
fn share_bookmark_meta(content_item_id: Uuid, raw_url: &str) -> CoreBookmarkMeta {
    CoreBookmarkMeta {
        content_item_id,
        original_url: Some(raw_url.to_owned()),
        saved_from: Some(SHARE_SHEET_SOURCE.to_owned()),
        thumbnail_url: None,
        description: None,
        site_name: None,
        favicon_url: None,
    }
}

/// Inserts the capture's selection as a highlight whose `ContentItem` id is the
/// `capture_id`, which is what makes finalization idempotent: reprocessing a
/// drop file that survived a crash finds the highlight already present and
/// inserts nothing. `source_item_id` links the highlight to its URL item, or is
/// `None` for a standalone text capture. Returns the highlight id and whether it
/// was newly inserted.
fn stage_share_highlight(
    db: &Database,
    capture_id: Uuid,
    source_item_id: Option<Uuid>,
    quote: &str,
    captured_at: OffsetDateTime,
) -> Result<(Uuid, bool), PergamonError> {
    match db.get_content_item(capture_id) {
        Ok(_) => return Ok((capture_id, false)),
        Err(StorageError::NotFound { .. }) => {}
        Err(e) => return Err(e.into()),
    }

    let item = CoreContentItem {
        id: capture_id,
        url: None,
        title: share_highlight_title(quote),
        author: None,
        content_type: CoreContentType::Highlight,
        status: CoreStatus::Inbox,
        content_text: Some(quote.to_owned()),
        excerpt: None,
        published_at: None,
        created_at: captured_at,
        updated_at: captured_at,
        read_at: None,
    };
    db.insert_content_item(&item)?;
    db.insert_highlight_meta(&CoreHighlightMeta {
        content_item_id: capture_id,
        source_item_id,
        quote_text: quote.to_owned(),
        note: None,
        position_start: None,
        position_end: None,
        color: None,
    })?;
    Ok((capture_id, true))
}

/// First-line, ≤80-char title for a share-captured highlight, mirroring
/// storage's `truncate_for_title` so it reads like a CLI/TUI-captured one.
fn share_highlight_title(quote: &str) -> String {
    let first_line = quote.lines().next().unwrap_or(quote);
    if first_line.chars().count() <= 80 {
        first_line.to_owned()
    } else {
        let truncated: String = first_line.chars().take(77).collect();
        format!("{truncated}…")
    }
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

/// Whether a core content item is a *document* (anything the app lists in the
/// inbox/library) rather than a highlight.
///
/// Highlights are stored as `content_items` (type `highlight`) with a linked
/// `highlight_meta` row, so every document-facing read path must filter them out
/// or the inbox and per-tag/collection counts would double-count annotations.
fn is_document(item: &CoreContentItem) -> bool {
    item.content_type != CoreContentType::Highlight
}

/// Whether an FFI content item matches the (already lowercased) search `needle`
/// across title, author, excerpt, URL, and extracted content. An empty needle
/// matches everything (facets do the narrowing in that case).
fn item_text_matches(item: &ContentItem, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hit = |field: Option<&String>| field.is_some_and(|v| v.to_lowercase().contains(needle));
    item.title.to_lowercase().contains(needle)
        || hit(item.author.as_ref())
        || hit(item.excerpt.as_ref())
        || hit(item.url.as_ref())
        || hit(item.content_text.as_ref())
}

/// Whether an FFI content item satisfies every active facet (AND-combined).
fn item_facets_match(item: &ContentItem, facets: &SearchFacets) -> bool {
    if facets
        .content_type
        .is_some_and(|ct| item.content_type != ct)
    {
        return false;
    }
    if facets.status.is_some_and(|status| item.status != status) {
        return false;
    }
    if let Some(tag) = facets.tag.as_ref().filter(|t| !t.trim().is_empty()) {
        let key = normalize_tag(tag);
        if !item.tags.iter().any(|t| normalize_tag(t) == key) {
            return false;
        }
    }
    if facets
        .source
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .is_some_and(|source| item.source_name.as_deref() != Some(source.as_str()))
    {
        return false;
    }
    date_range_matches(item.published_at_millis, facets)
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

/// Whether the search request has at least one active facet.
fn has_active_facets(facets: &SearchFacets) -> bool {
    facets.content_type.is_some()
        || facets.status.is_some()
        || facets.tag.as_ref().is_some_and(|t| !t.trim().is_empty())
        || facets.source.as_ref().is_some_and(|s| !s.trim().is_empty())
        || facets.since_millis.is_some()
        || facets.before_millis.is_some()
}

/// The 0-based nesting depth of a collection (root = 0), walking `parent_id`.
/// Guards against cycles by capping at the number of collections.
fn collection_depth(collections: &[CoreCollection], id: Uuid) -> u32 {
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

/// Precomputed per-item organization lookups (source name, tag names, collection
/// ids), built once per read so list assembly avoids per-item queries.
struct Lookups {
    /// content item id → feed title (the FFI `source_name`).
    source_names: HashMap<Uuid, String>,
    /// content item id → tag display names, sorted case-insensitively.
    tag_names: HashMap<Uuid, Vec<String>>,
    /// content item id → collection id strings, ordered by sort order.
    collection_ids: HashMap<Uuid, Vec<String>>,
}

impl Lookups {
    /// Builds the lookups from the whole library in a handful of bulk queries.
    fn build(db: &Database) -> Result<Self, StorageError> {
        let feed_titles: HashMap<Uuid, String> = db
            .list_feeds()?
            .into_iter()
            .map(|f| (f.id, f.title))
            .collect();
        let mut source_names = HashMap::new();
        for meta in db.list_all_feed_item_meta()? {
            if let Some(title) = feed_titles.get(&meta.feed_id) {
                source_names.insert(meta.content_item_id, title.clone());
            }
        }

        let tag_names_by_id: HashMap<Uuid, String> = db
            .list_tags()?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();
        let mut tag_names: HashMap<Uuid, Vec<String>> = HashMap::new();
        for (item_id, tag_id) in db.list_all_content_item_tags()? {
            if let Some(name) = tag_names_by_id.get(&tag_id) {
                tag_names.entry(item_id).or_default().push(name.clone());
            }
        }
        for names in tag_names.values_mut() {
            names.sort_by_key(|a| a.to_lowercase());
        }

        let mut coll_rows: HashMap<Uuid, Vec<(i32, Uuid)>> = HashMap::new();
        for (item_id, coll_id, sort) in db.list_all_collection_items()? {
            coll_rows.entry(item_id).or_default().push((sort, coll_id));
        }
        let mut collection_ids: HashMap<Uuid, Vec<String>> = HashMap::new();
        for (item_id, mut rows) in coll_rows {
            rows.sort_by_key(|(sort, _)| *sort);
            collection_ids.insert(
                item_id,
                rows.into_iter().map(|(_, id)| id.to_string()).collect(),
            );
        }

        Ok(Self {
            source_names,
            tag_names,
            collection_ids,
        })
    }

    /// Builds the FFI [`ContentItem`] view for a core item, folding in the
    /// FFI-only source name, tags, and collection ids.
    fn item_view(&self, core: &CoreContentItem) -> ContentItem {
        let reading_minutes = core
            .content_text
            .as_deref()
            .map_or(0, reading_time_from_text);
        ContentItem {
            id: core.id.to_string(),
            title: core.title.clone(),
            url: core.url.clone(),
            author: core.author.clone(),
            content_type: core.content_type.into(),
            status: core.status.into(),
            excerpt: core.excerpt.clone(),
            content_text: core.content_text.clone(),
            source_name: self.source_names.get(&core.id).cloned(),
            published_at_millis: core.published_at.map(millis),
            read_at_millis: core.read_at.map(millis),
            reading_minutes,
            tags: self.tag_names.get(&core.id).cloned().unwrap_or_default(),
            collection_ids: self
                .collection_ids
                .get(&core.id)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// Assembles the document (non-highlight) items for a status filter, newest
/// first, with organization fields folded in.
fn list_documents(
    db: &Database,
    status: Option<CoreStatus>,
) -> Result<Vec<ContentItem>, StorageError> {
    let lookups = Lookups::build(db)?;
    let items = match status {
        Some(st) => db.list_content_items(None, Some(st), None, None)?,
        None => db.list_all_content_items()?,
    };
    Ok(items
        .iter()
        .filter(|i| is_document(i))
        .map(|i| lookups.item_view(i))
        .collect())
}

/// Reads a single item and folds in its organization fields.
fn view_item(db: &Database, id: Uuid) -> Result<ContentItem, StorageError> {
    let core = db.get_content_item(id)?;
    let lookups = Lookups::build(db)?;
    Ok(lookups.item_view(&core))
}

/// Builds the FFI [`ReviewCardView`] by joining a card with its backing
/// highlight. Returns `None` for a dangling card (highlight gone), which callers
/// filter out.
fn card_view(db: &Database, card: &CoreReviewCard) -> Result<Option<ReviewCardView>, StorageError> {
    let meta = match db.get_highlight_meta(card.content_item_id) {
        Ok(meta) => meta,
        Err(StorageError::NotFound { .. }) => return Ok(None),
        Err(err) => return Err(err),
    };
    let source_title = source_title(db, meta.source_item_id);
    Ok(Some(ReviewCardView {
        card_id: card.id.to_string(),
        highlight_id: card.content_item_id.to_string(),
        item_id: meta
            .source_item_id
            .map(|s| s.to_string())
            .unwrap_or_default(),
        quote_text: meta.quote_text,
        note: meta.note,
        source_title,
        state: card.state.into(),
        due_at_millis: millis(card.due_at),
        review_count: u32::try_from(card.review_count).unwrap_or(0),
        last_reviewed_at_millis: card.last_reviewed_at.map(millis),
    }))
}

/// The display title of a highlight's source document, denormalized into the FFI
/// views so a queue card renders without a second lookup. Falls back to a
/// placeholder for an orphaned reference.
fn source_title(db: &Database, source_item_id: Option<Uuid>) -> String {
    source_item_id
        .and_then(|id| db.get_content_item(id).ok())
        .map_or_else(|| "Unknown source".to_owned(), |item| item.title)
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

impl From<StorageError> for PergamonError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::NotFound { .. } => Self::NotFound {
                message: err.to_string(),
            },
            StorageError::Domain(core) => core.into(),
            StorageError::Constraint(message) => Self::InvalidInput { message },
            other => Self::Storage {
                message: other.to_string(),
            },
        }
    }
}

/// Record counts from a completed backup export or restore, surfaced to the app
/// so it can confirm how much data moved.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BackupSummary {
    /// Number of feeds.
    pub feeds: u32,
    /// Number of content items (documents and highlights).
    pub content_items: u32,
    /// Number of tags.
    pub tags: u32,
    /// Number of collections.
    pub collections: u32,
    /// Number of standalone notes.
    pub notes: u32,
    /// Number of spaced-repetition review cards.
    pub review_cards: u32,
    /// Total records moved across every table.
    pub total: u32,
}

impl From<BackupStats> for BackupSummary {
    fn from(stats: BackupStats) -> Self {
        let cast = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Self {
            feeds: cast(stats.feeds),
            content_items: cast(stats.content_items),
            tags: cast(stats.tags),
            collections: cast(stats.collections),
            notes: cast(stats.notes),
            review_cards: cast(stats.review_cards),
            total: cast(stats.total),
        }
    }
}

/// Provenance and size information about the backing store, for the app's
/// settings/diagnostics surface.
#[derive(Debug, Clone, uniffi::Record)]
pub struct StorageInfo {
    /// Current database schema (migration) version.
    pub schema_version: u32,
    /// Number of document (non-highlight) content items.
    pub document_count: u32,
    /// Number of captured highlights.
    pub highlight_count: u32,
}

/// Writes the seeded demo corpus into an (empty) database.
///
/// Uses fixed UUIDs and timestamps so [`Library::item`] is deterministic across
/// runs and tests. Seeds four feeds (providing the source names), five documents
/// linked to their feeds, the tag and collection registries with memberships,
/// and two highlights — each with a New review card due in the past so the
/// review queue is populated on first launch.
#[allow(clippy::too_many_lines)] // a flat, readable table of seed rows
fn seed(db: &Database) -> Result<(), StorageError> {
    fn at(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_item(
        db: &Database,
        n: u128,
        title: &str,
        url: &str,
        author: Option<&str>,
        content_type: CoreContentType,
        status: CoreStatus,
        excerpt: &str,
        text: &str,
        published: i64,
        feed_id: Option<Uuid>,
    ) -> Result<(), StorageError> {
        let created = at(published + 60);
        let item = CoreContentItem {
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
        db.insert_content_item(&item)?;
        if let Some(feed_id) = feed_id {
            db.insert_feed_item_meta(&CoreFeedItemMeta {
                content_item_id: item.id,
                feed_id,
                guid: None,
                summary: None,
            })?;
        }
        Ok(())
    }

    let feed_created = at(1_577_836_800);
    let ink_switch = Uuid::from_u128(2001);
    let memory_weekly = Uuid::from_u128(2002);
    let rust_mobile = Uuid::from_u128(2003);
    let reader_diaries = Uuid::from_u128(2004);
    let feeds = [
        (
            ink_switch,
            "Ink & Switch",
            "https://www.inkandswitch.com/feed.xml",
        ),
        (
            memory_weekly,
            "Memory Weekly",
            "https://example.org/memory-weekly.xml",
        ),
        (
            rust_mobile,
            "Rust Mobile Weekly",
            "https://example.org/rust-mobile.xml",
        ),
        (
            reader_diaries,
            "Reader Diaries",
            "https://example.org/reader-diaries.xml",
        ),
    ];
    for (id, title, url) in feeds {
        db.insert_feed(&CoreFeed {
            id,
            title: title.to_owned(),
            url: url.to_owned(),
            site_url: None,
            description: None,
            etag: None,
            last_modified_header: None,
            error_count: 0,
            last_error: None,
            last_fetched_at: None,
            folder_id: None,
            created_at: feed_created,
            updated_at: feed_created,
        })?;
    }

    insert_item(
        db,
        1,
        "Local-first software: you own your data",
        "https://www.inkandswitch.com/local-first/",
        Some("Ink & Switch"),
        CoreContentType::Article,
        CoreStatus::Inbox,
        "Seven ideals for software that keeps your data on your own devices.",
        &"word ".repeat(620),
        1_577_836_800,
        Some(ink_switch),
    )?;
    insert_item(
        db,
        2,
        "Designing a spaced-repetition scheduler with FSRS",
        "https://example.org/fsrs-deep-dive",
        Some("A. Researcher"),
        CoreContentType::Article,
        CoreStatus::Later,
        "How the Free Spaced Repetition Scheduler models memory stability.",
        &"word ".repeat(1400),
        1_609_459_200,
        Some(memory_weekly),
    )?;
    insert_item(
        db,
        3,
        "The Rust + UniFFI mobile toolchain",
        "https://example.org/rust-uniffi-mobile",
        Some("M. Mobile"),
        CoreContentType::FeedItem,
        CoreStatus::Reading,
        "Sharing a Rust core across iOS and Android without hand-written FFI.",
        &"word ".repeat(300),
        1_640_995_200,
        Some(rust_mobile),
    )?;
    insert_item(
        db,
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
    )?;
    insert_item(
        db,
        5,
        "Why I switched from Inoreader",
        "https://example.org/switching",
        Some("Power User"),
        CoreContentType::Bookmark,
        CoreStatus::Archived,
        "A migration story toward a unified, local-first reading workflow.",
        &"word ".repeat(210),
        1_704_067_200,
        Some(reader_diaries),
    )?;

    // Tag registry + membership. `get_or_create_tag` keeps the first-seen form.
    let tag_ids: HashMap<&str, Uuid> = ["local-first", "reading", "memory", "rust", "ios"]
        .into_iter()
        .map(|name| Ok((name, db.get_or_create_tag(name)?.id)))
        .collect::<Result<_, StorageError>>()?;
    let tag = |item: u128, name: &str| -> Result<(), StorageError> {
        db.tag_content_item(Uuid::from_u128(item), tag_ids[name])
    };
    tag(1, "local-first")?;
    tag(1, "reading")?;
    tag(2, "memory")?;
    tag(2, "reading")?;
    tag(3, "rust")?;
    tag(3, "ios")?;
    tag(5, "reading")?;

    // Collections: Deep Dives nests under Reading List; Tech is a second root.
    let reading_list = Uuid::from_u128(101);
    let deep_dives = Uuid::from_u128(102);
    let tech = Uuid::from_u128(103);
    let coll_created = at(1_577_836_800);
    let insert_collection =
        |id: Uuid, name: &str, parent: Option<Uuid>| -> Result<(), StorageError> {
            db.insert_collection(&CoreCollection {
                id,
                name: name.to_owned(),
                parent_id: parent,
                sort_order: 0,
                is_smart: false,
                filter_query: None,
                created_at: coll_created,
                updated_at: coll_created,
            })
        };
    insert_collection(reading_list, "Reading List", None)?;
    insert_collection(deep_dives, "Deep Dives", Some(reading_list))?;
    insert_collection(tech, "Tech", None)?;
    db.add_to_collection(Uuid::from_u128(1), reading_list, 0)?;
    db.add_to_collection(Uuid::from_u128(2), deep_dives, 0)?;
    db.add_to_collection(Uuid::from_u128(3), tech, 0)?;
    db.add_to_collection(Uuid::from_u128(5), reading_list, 0)?;

    // Highlights + a New review card each, due in the past so both are due now.
    let due = at(1_577_836_800);
    let hl_local = db.create_highlight(
        Uuid::from_u128(1),
        "You own your data, in spite of the cloud.",
        Some("The core promise of local-first software."),
        None,
    )?;
    let hl_fsrs = db.create_highlight(
        Uuid::from_u128(2),
        "FSRS models memory as stability and difficulty.",
        None,
        None,
    )?;
    for (n, highlight_item_id) in [(401u128, hl_local.id), (402u128, hl_fsrs.id)] {
        db.insert_review_card(&CoreReviewCard {
            id: Uuid::from_u128(n),
            content_item_id: highlight_item_id,
            state: CardState::New,
            stability: None,
            difficulty: None,
            due_at: due,
            last_reviewed_at: None,
            review_count: 0,
            lapse_count: 0,
            scheduled_days: None,
            created_at: due,
            updated_at: due,
        })?;
    }

    Ok(())
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
/// triages the library. It owns the on-device SQLite database
/// (`pergamon-storage`) behind a `Mutex` (a rusqlite `Connection` is `Send` but
/// not `Sync`), so reads (`inbox`, `items`, ...) and triage mutations
/// (`mark_read`, `archive`, `save_for_later`, ...) are safe to call from any
/// thread and persist across launches in the shared App Group container
/// (#118 / ADR-020).
///
/// Calls are **synchronous and blocking** by design (ADR-019): core logic and
/// local-DB access do not wait on anything, so the app invokes these off the
/// main actor rather than paying for `async`.
#[derive(uniffi::Object)]
pub struct Library {
    db: Mutex<Database>,
    /// Remote-sync session, populated by [`Library::configure_sync`].
    ///
    /// `None` until the app configures sync with its keychain-held account key.
    /// Held behind a `Mutex` so the `#[uniffi::Object]` stays `Sync` and so the
    /// scheduler's backoff state survives across background-refresh calls.
    sync: Mutex<Option<SyncSession>>,
}

/// A configured remote-sync session: the HTTP-backed engine plus the local
/// backoff scheduler that produces the next-wake hint for iOS `BGTaskScheduler`.
struct SyncSession {
    engine: pergamon_sync::SyncEngine<pergamon_sync::http::HttpTransport>,
    blobs: pergamon_sync::MemoryBlobStore,
    scheduler: pergamon_sync::SyncScheduler,
    jitter: pergamon_sync::Jitter,
}

/// Base backoff delay after the first offline/transient failure.
const SYNC_BACKOFF_BASE: Duration = Duration::from_secs(5);
/// Ceiling the backoff delay is clamped to (5 minutes).
#[allow(clippy::duration_suboptimal_units)]
const SYNC_BACKOFF_MAX: Duration = Duration::from_secs(300);
/// Backoff growth factor per consecutive failure.
const SYNC_BACKOFF_MULTIPLIER: f64 = 2.0;
/// Healthy-state cadence hint (15 minutes) — the floor iOS enforces for
/// `BGAppRefreshTask`, so a shorter interval would be ignored anyway.
#[allow(clippy::duration_suboptimal_units)]
const SYNC_REFRESH_INTERVAL: Duration = Duration::from_secs(900);

/// Outcome of a single background refresh, surfaced to the iOS
/// `BGTaskScheduler` handler so it can schedule the next wake.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BackgroundRefreshResult {
    /// Number of local changes pushed to the relay this round.
    pub pushed: u32,
    /// Number of remote changes applied to the local library this round.
    pub applied: u32,
    /// `true` when the round hit an offline/transient failure and backed off.
    ///
    /// The round still completed cleanly (no error); the app should simply
    /// reschedule using `retry_after_seconds` rather than treat it as failure.
    pub offline: bool,
    /// Suggested delay before the next background refresh, in seconds. On
    /// success this is the healthy cadence; when `offline`, the backoff delay.
    pub retry_after_seconds: u64,
}

impl Library {
    /// Locks the database, recovering from a poisoned mutex.
    ///
    /// A panic in another thread while the lock was held could poison it; the
    /// SQLite connection has no broken in-memory invariants of our own, so we
    /// recover the guard rather than propagate the poison.
    fn lock(&self) -> MutexGuard<'_, Database> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[uniffi::export]
impl Library {
    /// Opens an **in-memory**, seeded library.
    ///
    /// Deterministic (fixed UUIDs and timestamps) and non-persistent — used by
    /// tests, SwiftUI previews, and the macOS host smoke test. The app itself
    /// opens the persistent store with [`Self::open`].
    ///
    /// # Panics
    ///
    /// Panics only if the in-memory SQLite database cannot be opened or seeded,
    /// which indicates a build/linking fault rather than a runtime condition.
    #[uniffi::constructor]
    #[must_use]
    pub fn new() -> Arc<Self> {
        let db = Database::open_in_memory().expect("in-memory database must open");
        seed(&db).expect("seeding the in-memory demo library must succeed");
        Arc::new(Self {
            db: Mutex::new(db),
            sync: Mutex::new(None),
        })
    }

    /// Opens (creating and migrating if needed) the on-device SQLite library at
    /// `path`, seeding the demo corpus on first launch (an empty database).
    ///
    /// This is the persistent store the iOS app opens against its App Group
    /// container (#118 / ADR-020).
    ///
    /// # Errors
    ///
    /// [`PergamonError::Storage`] if the database cannot be opened, migrated, or
    /// seeded.
    #[uniffi::constructor]
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn open(path: String) -> Result<Arc<Self>, PergamonError> {
        let db = Database::open(Path::new(&path))?;
        if db.is_empty()? {
            seed(&db)?;
        }
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            sync: Mutex::new(None),
        }))
    }

    /// Returns every item in triage-`Inbox` status (the primary landing screen).
    #[must_use]
    pub fn inbox(&self) -> Vec<ContentItem> {
        self.items_with_status(Status::Inbox)
    }

    /// Returns all documents in the library (the "list" path). Highlights are
    /// excluded — they surface through [`Self::highlights`].
    #[must_use]
    pub fn items(&self) -> Vec<ContentItem> {
        let db = self.lock();
        list_documents(&db, None).unwrap_or_default()
    }

    /// Returns documents filtered to a single triage [`Status`].
    #[must_use]
    pub fn items_with_status(&self, status: Status) -> Vec<ContentItem> {
        let db = self.lock();
        list_documents(&db, Some(status.into())).unwrap_or_default()
    }

    /// Returns the distinct feed/source names present in the library, sorted
    /// alphabetically. Drives the inbox's feed filter.
    #[must_use]
    pub fn sources(&self) -> Vec<String> {
        let db = self.lock();
        let mut names: Vec<String> = list_documents(&db, None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| item.source_name)
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Fetches a single item by its UUID string (the "open" path).
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] if `id` is not a valid UUID, or
    /// [`PergamonError::NotFound`] if no item with that id exists.
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn item(&self, id: String) -> Result<ContentItem, PergamonError> {
        let wanted = parse_uuid(&id)?;
        let db = self.lock();
        Ok(view_item(&db, wanted)?)
    }

    /// Returns documents whose title, author, excerpt, URL, or extracted content
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
    /// query with active facets returns every document passing the facets.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn search_filtered(&self, query: String, facets: SearchFacets) -> Vec<ContentItem> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() && !has_active_facets(&facets) {
            return Vec::new();
        }
        let db = self.lock();
        list_documents(&db, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item_text_matches(item, &needle) && item_facets_match(item, &facets))
            .collect()
    }

    /// Returns every tag in the registry with its current document count, sorted
    /// by name.
    #[must_use]
    pub fn tags(&self) -> Vec<Tag> {
        let db = self.lock();
        tags_impl(&db).unwrap_or_default()
    }

    /// Returns every collection with its direct document count and precomputed
    /// nesting depth, ordered so each parent precedes its children (a
    /// depth-first tree order suitable for indented rendering).
    #[must_use]
    pub fn collections(&self) -> Vec<Collection> {
        let db = self.lock();
        collections_impl(&db).unwrap_or_default()
    }

    /// Returns documents carrying `tag` (case-insensitive). An empty tag matches
    /// nothing.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn items_with_tag(&self, tag: String) -> Vec<ContentItem> {
        let key = normalize_tag(&tag);
        if key.is_empty() {
            return Vec::new();
        }
        let db = self.lock();
        list_documents(&db, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.tags.iter().any(|t| normalize_tag(t) == key))
            .collect()
    }

    /// Returns documents directly in the collection with `collection_id`.
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
        let db = self.lock();
        collection_or_not_found(&db, wanted, &collection_id)?;
        let target = wanted.to_string();
        Ok(list_documents(&db, None)?
            .into_iter()
            .filter(|item| item.collection_ids.iter().any(|c| c == &target))
            .collect())
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
        let db = self.lock();
        item_or_not_found(&db, wanted, &id)?;
        let tag = db.get_or_create_tag(&display)?;
        db.tag_content_item(wanted, tag.id)?;
        Ok(view_item(&db, wanted)?)
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
        let wanted = parse_uuid(&id)?;
        let key = normalize_tag(&name);
        let db = self.lock();
        item_or_not_found(&db, wanted, &id)?;
        if !key.is_empty()
            && let Some(tag) = db
                .list_tags()?
                .into_iter()
                .find(|t| normalize_tag(&t.name) == key)
        {
            db.untag_content_item(wanted, tag.id)?;
        }
        Ok(view_item(&db, wanted)?)
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
        let db = self.lock();
        if let Some(parent) = parent {
            collection_or_not_found(&db, parent, &parent.to_string())?;
        }
        let now = OffsetDateTime::now_utc();
        let id = Uuid::new_v4();
        db.insert_collection(&CoreCollection {
            id,
            name: display.clone(),
            parent_id: parent,
            sort_order: 0,
            is_smart: false,
            filter_query: None,
            created_at: now,
            updated_at: now,
        })?;
        let depth = collection_depth(&db.list_collections()?, id);
        Ok(Collection {
            id: id.to_string(),
            name: display,
            parent_id: parent.map(|p| p.to_string()),
            item_count: 0,
            depth,
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
        let db = self.lock();
        collection_or_not_found(&db, coll_id, &collection_id)?;
        item_or_not_found(&db, item_id, &id)?;
        db.add_to_collection(item_id, coll_id, 0)?;
        Ok(view_item(&db, item_id)?)
    }

    /// Removes the item from the collection. Idempotent — removing an item not in
    /// the collection is a no-op. Returns the updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when the item or collection does not exist.
    #[allow(clippy::needless_pass_by_value)]
    pub fn remove_from_collection(
        &self,
        id: String,
        collection_id: String,
    ) -> Result<ContentItem, PergamonError> {
        let item_id = parse_uuid(&id)?;
        let coll_id = parse_uuid(&collection_id)?;
        let db = self.lock();
        item_or_not_found(&db, item_id, &id)?;
        collection_or_not_found(&db, coll_id, &collection_id)?;
        db.remove_from_collection(item_id, coll_id)?;
        Ok(view_item(&db, item_id)?)
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
        let wanted = parse_uuid(&id)?;
        let db = self.lock();
        let core = db.get_content_item(wanted)?;
        if core.read_at.is_none() {
            db.set_content_item_read_at(wanted, Some(OffsetDateTime::now_utc()))?;
        }
        Ok(view_item(&db, wanted)?)
    }

    /// Marks the item unread, clearing `read_at`. Returns the updated item.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] for a malformed id, or
    /// [`PergamonError::NotFound`] when no item matches.
    #[allow(clippy::needless_pass_by_value)]
    pub fn mark_unread(&self, id: String) -> Result<ContentItem, PergamonError> {
        let wanted = parse_uuid(&id)?;
        let db = self.lock();
        db.set_content_item_read_at(wanted, None)?;
        Ok(view_item(&db, wanted)?)
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
        let wanted = parse_uuid(&id)?;
        let db = self.lock();
        db.update_content_item_status(wanted, CoreStatus::Archived)?;
        Ok(view_item(&db, wanted)?)
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
        let wanted = parse_uuid(&id)?;
        let db = self.lock();
        db.update_content_item_status(wanted, CoreStatus::Later)?;
        Ok(view_item(&db, wanted)?)
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
        let db = self.lock();
        let source = db.get_content_item(wanted)?;
        let mut pairs = db.list_highlights(Some(wanted), None, None, None, None)?;
        pairs.sort_by_key(|(item, _)| item.created_at);
        let mut out = Vec::with_capacity(pairs.len());
        for (item, meta) in pairs {
            let has_review_card = db.get_review_card_for_item(item.id)?.is_some();
            out.push(Highlight {
                id: item.id.to_string(),
                item_id: meta
                    .source_item_id
                    .map_or_else(|| wanted.to_string(), |s| s.to_string()),
                quote_text: meta.quote_text,
                note: meta.note,
                source_title: source.title.clone(),
                created_at_millis: millis(item.created_at),
                has_review_card,
            });
        }
        Ok(out)
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
        let db = self.lock();
        let source = db.get_content_item(wanted)?;
        let highlight = db.create_highlight(wanted, &quote, note.as_deref(), None)?;
        let now = OffsetDateTime::now_utc();
        db.insert_review_card(&CoreReviewCard {
            id: Uuid::new_v4(),
            content_item_id: highlight.id,
            state: CardState::New,
            stability: None,
            difficulty: None,
            due_at: now,
            last_reviewed_at: None,
            review_count: 0,
            lapse_count: 0,
            scheduled_days: None,
            created_at: now,
            updated_at: now,
        })?;
        Ok(Highlight {
            id: highlight.id.to_string(),
            item_id: wanted.to_string(),
            quote_text: quote,
            note,
            source_title: source.title,
            created_at_millis: millis(highlight.created_at),
            has_review_card: true,
        })
    }

    /// Finalizes one staged share-sheet [`ShareCapture`] into the library,
    /// running the **same** ingestion pipeline as CLI `save` and web capture per
    /// **ADR-021**: canonicalize the URL, dedupe on the canonical URL, create or
    /// reuse the `ContentItem` (attaching `BookmarkMeta` with
    /// `saved_from = "share-sheet"` and the raw shared URL), and attach any
    /// selection as a `Highlight`. A text-only capture becomes a standalone
    /// highlight.
    ///
    /// The share extension itself does no network, extraction, or database work
    /// (that would blow its memory/time budget); this is the app-side finalizer
    /// it hands off to. The call is **idempotent** and crash-safe: a URL capture
    /// dedupes on the canonical URL and its highlight on the `capture_id`, and a
    /// text-only capture dedupes on the `capture_id`, so reprocessing a drop file
    /// that survived a crash converges on the same rows instead of duplicating
    /// them. Extraction (fetch + readability) is deferred — a URL capture lands
    /// as a bookmark and upgrades to an article in place later (ADR-010).
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] when `capture_id` is not a valid UUID or
    /// the capture carries neither a usable URL nor selection text, or
    /// [`PergamonError::Storage`] on a database failure.
    #[allow(clippy::needless_pass_by_value)]
    pub fn ingest_share_capture(
        &self,
        capture: ShareCapture,
    ) -> Result<ShareIngestOutcome, PergamonError> {
        let capture_id = parse_uuid(&capture.capture_id)?;
        let captured_at = from_millis(capture.captured_at_millis);
        let raw_url = capture
            .url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty());
        let selection = capture
            .selected_text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let db = self.lock();

        // URL-backed capture: canonicalize → dedupe → create/reuse the item,
        // then attach the selection (if any) as a linked highlight.
        if let Some(raw) = raw_url {
            // The exact canonicalization CLI and web use, so dedupe is identical
            // across every surface (ADR-021).
            let canonical =
                pergamon_extract::canonicalize_url(raw).unwrap_or_else(|_| raw.to_owned());

            let (item_id, item_inserted) =
                if let Some(existing) = db.get_content_item_by_url(&canonical)? {
                    // Merge on duplicate: enrich provenance without clobbering.
                    // `upsert_bookmark_meta` COALESCEs, so existing values win.
                    db.upsert_bookmark_meta(&share_bookmark_meta(existing.id, raw))?;
                    (existing.id, false)
                } else {
                    let title = capture
                        .page_title
                        .as_deref()
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map_or_else(|| raw.to_owned(), ToOwned::to_owned);
                    let item = CoreContentItem {
                        id: Uuid::new_v4(),
                        url: Some(canonical),
                        title,
                        author: None,
                        content_type: CoreContentType::Bookmark,
                        status: CoreStatus::Inbox,
                        content_text: None,
                        excerpt: None,
                        published_at: None,
                        created_at: captured_at,
                        updated_at: captured_at,
                        read_at: None,
                    };
                    db.insert_content_item(&item)?;
                    db.insert_bookmark_meta(&share_bookmark_meta(item.id, raw))?;
                    (item.id, true)
                };

            let (highlight_id, hl_inserted) = if let Some(quote) = selection {
                let (id, inserted) =
                    stage_share_highlight(&db, capture_id, Some(item_id), quote, captured_at)?;
                (Some(id.to_string()), inserted)
            } else {
                (None, false)
            };

            return Ok(ShareIngestOutcome {
                item_id: Some(item_id.to_string()),
                highlight_id,
                deduped: !(item_inserted || hl_inserted),
            });
        }

        // Text-only capture: a standalone highlight keyed on `capture_id`.
        if let Some(quote) = selection {
            let (highlight_id, inserted) =
                stage_share_highlight(&db, capture_id, None, quote, captured_at)?;
            return Ok(ShareIngestOutcome {
                item_id: None,
                highlight_id: Some(highlight_id.to_string()),
                deduped: !inserted,
            });
        }

        Err(PergamonError::InvalidInput {
            message: "share capture has neither a URL nor selection text".to_owned(),
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
        let db = self.lock();
        db.update_highlight_note(wanted, note.as_deref())?;
        let item = db.get_content_item(wanted)?;
        let meta = db.get_highlight_meta(wanted)?;
        let has_review_card = db.get_review_card_for_item(wanted)?.is_some();
        Ok(Highlight {
            id: wanted.to_string(),
            item_id: meta
                .source_item_id
                .map(|s| s.to_string())
                .unwrap_or_default(),
            quote_text: meta.quote_text,
            note: meta.note,
            source_title: source_title(&db, meta.source_item_id),
            created_at_millis: millis(item.created_at),
            has_review_card,
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
        let db = self.lock();
        db.delete_content_item(wanted)?;
        Ok(())
    }

    /// Returns the review cards currently due (`due_at <= now`), soonest-due
    /// first. This is the review queue the app grades through.
    #[must_use]
    pub fn due_cards(&self) -> Vec<ReviewCardView> {
        let db = self.lock();
        due_cards_impl(&db).unwrap_or_default()
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
        let db = self.lock();
        let card = db.get_review_card(wanted)?;

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
        let review_count = card.review_count + 1;
        let lapse_count = card.lapse_count + i32::from(rating == Rating::Again);

        db.update_review_card(
            wanted,
            output.next_state.as_str(),
            output.memory.stability,
            output.memory.difficulty,
            due_at,
            now,
            review_count,
            lapse_count,
            output.scheduled_days,
        )?;

        db.insert_review_log(&CoreReviewLog {
            id: Uuid::new_v4(),
            card_id: wanted,
            rating,
            state_before: card.state,
            stability_before: card.stability,
            difficulty_before: card.difficulty,
            state_after: output.next_state,
            stability_after: output.memory.stability,
            difficulty_after: output.memory.difficulty,
            elapsed_days,
            scheduled_days: output.scheduled_days,
            reviewed_at: now,
        })?;

        let updated = db.get_review_card(wanted)?;
        card_view(&db, &updated)?.ok_or(PergamonError::NotFound {
            message: format!("highlight for card {card_id} is gone"),
        })
    }

    /// Returns aggregate review counters for surfacing the due-count badge and
    /// queue health.
    #[must_use]
    pub fn review_summary(&self) -> ReviewSummary {
        let db = self.lock();
        let now = OffsetDateTime::now_utc();
        let cast = |n: i64| u32::try_from(n).unwrap_or(u32::MAX);
        db.review_stats(now).map_or(
            ReviewSummary {
                due_count: 0,
                total_cards: 0,
                new_count: 0,
                reviews_today: 0,
            },
            |stats| ReviewSummary {
                due_count: cast(stats.due_count),
                total_cards: cast(stats.total_cards),
                new_count: cast(stats.new_count),
                reviews_today: cast(stats.reviews_today),
            },
        )
    }

    // ---- Backup / restore (canonical, cross-client format) -------------------

    /// Exports the whole library to a canonical backup archive at `path` (a ZIP
    /// of JSON), the same format the CLI and web clients read and write. Returns
    /// the record counts moved.
    ///
    /// # Errors
    ///
    /// [`PergamonError::Storage`] if the file cannot be created or the library
    /// cannot be read.
    #[allow(clippy::needless_pass_by_value)]
    pub fn export_backup(&self, path: String) -> Result<BackupSummary, PergamonError> {
        let db = self.lock();
        let mut file = std::fs::File::create(&path).map_err(|e| PergamonError::Storage {
            message: format!("cannot create backup file {path}: {e}"),
        })?;
        let stats = backup::export(&db, &mut file)?;
        Ok(stats.into())
    }

    /// Restores a canonical backup archive from `path`, **replacing** the current
    /// library. The database is reset first, then the archive is restored, so a
    /// backup produced on any client (CLI, web, iOS) fully round-trips here.
    ///
    /// # Errors
    ///
    /// [`PergamonError::Storage`] if the file cannot be opened, is not a valid
    /// pergamon backup, or its schema is newer than this build supports.
    #[allow(clippy::needless_pass_by_value)]
    pub fn restore_backup(&self, path: String) -> Result<BackupSummary, PergamonError> {
        let db = self.lock();
        let mut file = std::fs::File::open(&path).map_err(|e| PergamonError::Storage {
            message: format!("cannot open backup file {path}: {e}"),
        })?;
        db.reset()?;
        let stats = backup::restore(&db, &mut file)?;
        Ok(stats.into())
    }

    /// Returns provenance and size information about the backing store.
    #[must_use]
    pub fn storage_info(&self) -> StorageInfo {
        let db = self.lock();
        storage_info_impl(&db).unwrap_or(StorageInfo {
            schema_version: 0,
            document_count: 0,
            highlight_count: 0,
        })
    }

    /// Configures end-to-end-encrypted remote sync for this library (issue #129).
    ///
    /// The app supplies the relay `server` URL, its account identity
    /// (`account_id_hex`, `device_id`), the 32-byte account root key
    /// (`account_root_key`, held in the iOS keychain and never persisted by this
    /// crate), and the active `key_epoch`. This builds the crypto context and
    /// HTTP transport once and stores them so [`Self::background_refresh`] can
    /// run cheap single-shot rounds. Calling it again replaces the session,
    /// e.g. after an epoch rotation.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] if the key is not 32 bytes,
    /// [`PergamonError::Network`] if the server URL is invalid, or
    /// [`PergamonError::Internal`] if the crypto context cannot be derived.
    #[allow(clippy::needless_pass_by_value)] // owned args are the idiomatic UniFFI signature
    pub fn configure_sync(
        &self,
        server: String,
        account_id_hex: String,
        device_id: String,
        account_root_key: Vec<u8>,
        key_epoch: u32,
    ) -> Result<(), PergamonError> {
        let ark_bytes: [u8; 32] =
            account_root_key
                .as_slice()
                .try_into()
                .map_err(|_| PergamonError::InvalidInput {
                    message: format!(
                        "account root key must be 32 bytes, got {}",
                        account_root_key.len()
                    ),
                })?;
        let ark = pergamon_crypto::hierarchy::AccountRootKey::from_bytes(ark_bytes);
        let crypto = pergamon_sync::CryptoContext::new(ark, account_id_hex, device_id, key_epoch)
            .map_err(|e| PergamonError::Internal {
            message: e.to_string(),
        })?;
        let transport = pergamon_sync::http::HttpTransport::new(server).map_err(|e| {
            PergamonError::Network {
                message: e.to_string(),
            }
        })?;
        let engine = pergamon_sync::SyncEngine::new(transport, crypto);
        let backoff = pergamon_sync::BackoffPolicy::new(
            SYNC_BACKOFF_BASE,
            SYNC_BACKOFF_MAX,
            SYNC_BACKOFF_MULTIPLIER,
        );
        let session = SyncSession {
            engine,
            blobs: pergamon_sync::MemoryBlobStore::new(),
            scheduler: pergamon_sync::SyncScheduler::new(SYNC_REFRESH_INTERVAL, backoff),
            jitter: pergamon_sync::Jitter::from_entropy(),
        };
        let mut guard = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
        *guard = Some(session);
        Ok(())
    }

    /// Reports whether remote sync has been configured via
    /// [`Self::configure_sync`].
    #[must_use]
    pub fn is_sync_configured(&self) -> bool {
        self.sync
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Runs one background sync round: push local changes, pull and apply remote
    /// ones (issue #129). Designed to be called from the iOS `BGTaskScheduler`
    /// handler; it is single-shot and blocking (ADR-019).
    ///
    /// Offline/transient failures are **not** errors: the round returns with
    /// `offline = true` and a backoff-derived `retry_after_seconds` so the app
    /// can reschedule. Only fatal (crypto/protocol) failures return an error.
    ///
    /// # Errors
    ///
    /// [`PergamonError::InvalidInput`] if sync is not configured, or
    /// [`PergamonError::Internal`] on a fatal, non-retryable sync failure.
    #[allow(clippy::significant_drop_tightening)] // both guards are needed for the whole round
    pub fn background_refresh(&self) -> Result<BackgroundRefreshResult, PergamonError> {
        let mut sync_guard = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
        let session = sync_guard
            .as_mut()
            .ok_or_else(|| PergamonError::InvalidInput {
                message: "remote sync is not configured; call configure_sync first".to_owned(),
            })?;
        let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        match session.engine.sync(&db, &session.blobs) {
            Ok(stats) => {
                session.scheduler.record_success();
                let next = session.scheduler.next_delay(session.jitter.next01());
                Ok(BackgroundRefreshResult {
                    pushed: u32::try_from(stats.pushed).unwrap_or(u32::MAX),
                    applied: u32::try_from(stats.applied).unwrap_or(u32::MAX),
                    offline: false,
                    retry_after_seconds: next.as_secs(),
                })
            }
            Err(e) if e.is_retryable() => {
                session.scheduler.record_failure();
                let next = session.scheduler.next_delay(session.jitter.next01());
                Ok(BackgroundRefreshResult {
                    pushed: 0,
                    applied: 0,
                    offline: true,
                    retry_after_seconds: next.as_secs(),
                })
            }
            Err(e) => Err(PergamonError::Internal {
                message: e.to_string(),
            }),
        }
    }
}

/// Assembles the tag registry with per-tag document counts.
fn tags_impl(db: &Database) -> Result<Vec<Tag>, StorageError> {
    let lookups = Lookups::build(db)?;
    let mut counts: HashMap<String, u32> = HashMap::new();
    for item in db
        .list_all_content_items()?
        .iter()
        .filter(|i| is_document(i))
    {
        if let Some(names) = lookups.tag_names.get(&item.id) {
            for name in names {
                *counts.entry(normalize_tag(name)).or_default() += 1;
            }
        }
    }
    let mut tags: Vec<Tag> = db
        .list_tags()?
        .into_iter()
        .map(|tag| {
            let item_count = counts.get(&normalize_tag(&tag.name)).copied().unwrap_or(0);
            Tag {
                id: tag.id.to_string(),
                name: tag.name,
                item_count,
            }
        })
        .collect();
    tags.sort_by_key(|a| a.name.to_lowercase());
    Ok(tags)
}

/// Assembles collections in depth-first tree order with direct document counts.
fn collections_impl(db: &Database) -> Result<Vec<Collection>, StorageError> {
    let colls = db.list_collections()?;
    let documents: HashSet<Uuid> = db
        .list_all_content_items()?
        .into_iter()
        .filter(is_document)
        .map(|i| i.id)
        .collect();
    let mut counts: HashMap<Uuid, u32> = HashMap::new();
    for (item_id, coll_id, _sort) in db.list_all_collection_items()? {
        if documents.contains(&item_id) {
            *counts.entry(coll_id).or_default() += 1;
        }
    }
    let mut out = Vec::with_capacity(colls.len());
    push_collection_children(&colls, &counts, None, 0, &mut out);
    Ok(out)
}

/// Depth-first walk pushing each parent immediately before its children, with
/// siblings ordered case-insensitively by name.
fn push_collection_children(
    colls: &[CoreCollection],
    counts: &HashMap<Uuid, u32>,
    parent: Option<Uuid>,
    depth: u32,
    out: &mut Vec<Collection>,
) {
    let mut children: Vec<&CoreCollection> =
        colls.iter().filter(|c| c.parent_id == parent).collect();
    children.sort_by_key(|a| a.name.to_lowercase());
    for child in children {
        out.push(Collection {
            id: child.id.to_string(),
            name: child.name.clone(),
            parent_id: child.parent_id.map(|p| p.to_string()),
            item_count: counts.get(&child.id).copied().unwrap_or(0),
            depth,
        });
        push_collection_children(colls, counts, Some(child.id), depth + 1, out);
    }
}

/// Assembles the due-review queue, skipping any dangling cards.
fn due_cards_impl(db: &Database) -> Result<Vec<ReviewCardView>, StorageError> {
    let now = OffsetDateTime::now_utc();
    let mut out = Vec::new();
    for card in db.list_due_review_cards(now)? {
        if let Some(view) = card_view(db, &card)? {
            out.push(view);
        }
    }
    Ok(out)
}

/// Computes [`StorageInfo`] from the current database.
fn storage_info_impl(db: &Database) -> Result<StorageInfo, StorageError> {
    let all = db.list_all_content_items()?;
    let highlight_count = all.iter().filter(|i| !is_document(i)).count();
    let document_count = all.len() - highlight_count;
    Ok(StorageInfo {
        schema_version: u32::try_from(db.schema_version()?).unwrap_or(0),
        document_count: u32::try_from(document_count).unwrap_or(u32::MAX),
        highlight_count: u32::try_from(highlight_count).unwrap_or(u32::MAX),
    })
}

/// Ensures a content item exists, mapping absence to [`PergamonError::NotFound`]
/// with a facade-shaped message.
fn item_or_not_found(db: &Database, id: Uuid, raw: &str) -> Result<(), PergamonError> {
    match db.get_content_item(id) {
        Ok(_) => Ok(()),
        Err(StorageError::NotFound { .. }) => Err(PergamonError::NotFound {
            message: format!("no item with id {raw}"),
        }),
        Err(other) => Err(other.into()),
    }
}

/// Ensures a collection exists, mapping absence to [`PergamonError::NotFound`]
/// with a facade-shaped message.
fn collection_or_not_found(db: &Database, id: Uuid, raw: &str) -> Result<(), PergamonError> {
    match db.get_collection(id) {
        Ok(_) => Ok(()),
        Err(StorageError::NotFound { .. }) => Err(PergamonError::NotFound {
            message: format!("no collection with id {raw}"),
        }),
        Err(other) => Err(other.into()),
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

    // ---- Share-sheet ingestion (ADR-021) -------------------------------------

    fn capture(
        kind: ShareContentKind,
        url: Option<&str>,
        text: Option<&str>,
        title: Option<&str>,
    ) -> ShareCapture {
        ShareCapture {
            capture_id: Uuid::new_v4().to_string(),
            captured_at_millis: 1_700_000_000_000,
            content_kind: kind,
            url: url.map(str::to_owned),
            selected_text: text.map(str::to_owned),
            page_title: title.map(str::to_owned),
            source_app: Some("com.apple.mobilesafari".to_owned()),
        }
    }

    #[test]
    fn ingest_share_capture_url_creates_inbox_bookmark() {
        let lib = library();
        let before = lib.items().len();

        let out = lib
            .ingest_share_capture(capture(
                ShareContentKind::Url,
                Some("https://example.com/post"),
                None,
                Some("A Great Post"),
            ))
            .expect("valid url capture");

        assert!(!out.deduped);
        assert!(out.highlight_id.is_none());
        let id = out.item_id.expect("url capture yields an item");

        let item = lib.item(id).expect("item exists");
        assert_eq!(item.content_type, ContentType::Bookmark);
        assert_eq!(item.status, Status::Inbox);
        assert_eq!(item.title, "A Great Post");
        assert_eq!(lib.items().len(), before + 1);
    }

    #[test]
    fn ingest_share_capture_falls_back_to_url_as_title() {
        let lib = library();
        let out = lib
            .ingest_share_capture(capture(
                ShareContentKind::Url,
                Some("https://example.com/no-title"),
                None,
                None,
            ))
            .expect("valid");
        let item = lib.item(out.item_id.expect("item")).expect("exists");
        assert_eq!(item.title, "https://example.com/no-title");
    }

    #[test]
    fn ingest_share_capture_dedupes_on_canonical_url() {
        let lib = library();
        let before = lib.items().len();

        let first = lib
            .ingest_share_capture(capture(
                ShareContentKind::Url,
                Some("https://example.com/a"),
                None,
                None,
            ))
            .expect("first");
        assert!(!first.deduped);

        // A tracking-param variant canonicalizes to the same URL, so it merges
        // onto the existing item rather than creating a second one.
        let second = lib
            .ingest_share_capture(capture(
                ShareContentKind::Url,
                Some("https://example.com/a?utm_source=newsletter"),
                None,
                None,
            ))
            .expect("second");
        assert!(second.deduped);
        assert_eq!(second.item_id, first.item_id);
        assert_eq!(lib.items().len(), before + 1);
    }

    #[test]
    fn ingest_share_capture_url_with_selection_attaches_highlight() {
        let lib = library();
        let out = lib
            .ingest_share_capture(capture(
                ShareContentKind::UrlWithSelection,
                Some("https://example.com/read"),
                Some("  a memorable sentence  "),
                Some("Readable"),
            ))
            .expect("valid");

        assert!(!out.deduped);
        let item_id = out.item_id.expect("url item");
        assert!(out.highlight_id.is_some());

        let highlights = lib.highlights(item_id).expect("item exists");
        assert_eq!(highlights.len(), 1);
        // Selection text is trimmed, matching CLI capture behavior.
        assert_eq!(highlights[0].quote_text, "a memorable sentence");
    }

    #[test]
    fn ingest_share_capture_text_creates_standalone_highlight() {
        let lib = library();
        let cap = capture(
            ShareContentKind::Text,
            None,
            Some("standalone thought"),
            None,
        );

        let out = lib.ingest_share_capture(cap.clone()).expect("valid");
        assert!(!out.deduped);
        assert!(out.item_id.is_none());
        let hl_id = out.highlight_id.expect("standalone highlight");
        // The highlight's id is the capture id (provenance / idempotency key).
        assert_eq!(hl_id, cap.capture_id);

        // Re-finalizing the same drop file (crash between commit and delete)
        // must converge, not duplicate.
        let again = lib.ingest_share_capture(cap).expect("valid");
        assert!(again.deduped);
        assert_eq!(again.highlight_id.as_deref(), Some(hl_id.as_str()));
    }

    #[test]
    fn ingest_share_capture_url_with_selection_is_idempotent() {
        let lib = library();
        let cap = capture(
            ShareContentKind::UrlWithSelection,
            Some("https://example.com/idem"),
            Some("quote once"),
            None,
        );

        let first = lib.ingest_share_capture(cap.clone()).expect("valid");
        let item_id = first.item_id.clone().expect("item");

        let second = lib.ingest_share_capture(cap).expect("valid");
        assert!(second.deduped);
        assert_eq!(second.item_id, first.item_id);
        // Still exactly one highlight, not two.
        assert_eq!(lib.highlights(item_id).expect("item").len(), 1);
    }

    #[test]
    fn ingest_share_capture_rejects_empty_and_malformed() {
        let lib = library();

        // Neither URL nor text.
        assert!(matches!(
            lib.ingest_share_capture(capture(ShareContentKind::Text, None, Some("   "), None)),
            Err(PergamonError::InvalidInput { .. })
        ));

        // Malformed capture id.
        let mut bad = capture(
            ShareContentKind::Url,
            Some("https://example.com"),
            None,
            None,
        );
        bad.capture_id = "not-a-uuid".to_owned();
        assert!(matches!(
            lib.ingest_share_capture(bad),
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
