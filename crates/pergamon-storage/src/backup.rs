//! Canonical backup archive format (ZIP of JSON), shared by every client.
//!
//! A pergamon backup is a ZIP archive whose entries are pretty-printed JSON
//! snapshots of each table in the library, plus a `manifest.json` describing the
//! app and schema version. Keeping the read/write logic here — rather than in
//! any single client — is what makes the format *canonical*: the CLI, the web
//! server, and the iOS app all round-trip the exact same bytes, so a backup
//! produced on one restores on another (issue #118, ADR-020 §4).
//!
//! The archive layout is:
//!
//! ```text
//! manifest.json          { app, schema_version, created_at }
//! feed_folders.json      [FeedFolder]
//! feeds.json             [Feed]
//! content_items.json     [ContentItem]
//! tags.json              [Tag]
//! collections.json       [Collection]
//! feed_item_meta.json    [FeedItemMeta]
//! bookmark_meta.json     [BookmarkMeta]
//! highlight_meta.json    [HighlightMeta]
//! notes.json             [Note]
//! review_cards.json      [ReviewCard]
//! review_logs.json       [ReviewLog]
//! content_rules.json     [ContentRule]
//! content_item_tags.json      [(content_item_id, tag_id)]
//! collection_items.json       [(content_item_id, collection_id, sort_order)]
//! ```

use std::io::{Read, Seek, Write};

use pergamon_core::model::{
    BookmarkMeta, Collection, ContentItem, Feed, FeedFolder, FeedItemMeta, HighlightMeta, Note,
    ReviewCard, ReviewLog, Tag,
};
use pergamon_core::rule::ContentRule;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::db::Database;
use crate::error::StorageError;

/// The `app` value every pergamon backup manifest carries.
pub const MANIFEST_APP: &str = "pergamon";

/// Manifest embedded in every backup archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Application name (always [`MANIFEST_APP`]).
    pub app: String,
    /// Schema version at the time of the backup.
    pub schema_version: i64,
    /// RFC-3339 timestamp of when the backup was created.
    pub created_at: String,
}

/// Record counts for a completed export or restore, for user-facing summaries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackupStats {
    /// Number of feeds.
    pub feeds: usize,
    /// Number of content items.
    pub content_items: usize,
    /// Number of tags.
    pub tags: usize,
    /// Number of collections.
    pub collections: usize,
    /// Number of notes.
    pub notes: usize,
    /// Number of review cards.
    pub review_cards: usize,
    /// Number of content rules.
    pub rules: usize,
    /// Total records written or read across every table.
    pub total: usize,
}

/// Names of the JSON entries, in deterministic write order.
mod entry {
    pub const MANIFEST: &str = "manifest.json";
    pub const FEED_FOLDERS: &str = "feed_folders.json";
    pub const FEEDS: &str = "feeds.json";
    pub const CONTENT_ITEMS: &str = "content_items.json";
    pub const TAGS: &str = "tags.json";
    pub const COLLECTIONS: &str = "collections.json";
    pub const FEED_ITEM_META: &str = "feed_item_meta.json";
    pub const BOOKMARK_META: &str = "bookmark_meta.json";
    pub const HIGHLIGHT_META: &str = "highlight_meta.json";
    pub const NOTES: &str = "notes.json";
    pub const REVIEW_CARDS: &str = "review_cards.json";
    pub const REVIEW_LOGS: &str = "review_logs.json";
    pub const CONTENT_RULES: &str = "content_rules.json";
    pub const CONTENT_ITEM_TAGS: &str = "content_item_tags.json";
    pub const COLLECTION_ITEMS: &str = "collection_items.json";
}

/// Write a full backup of `db` to `writer` as a canonical ZIP archive.
///
/// `writer` must be seekable (a file or an in-memory cursor). Returns the record
/// counts for a user-facing summary.
///
/// # Errors
///
/// Returns [`StorageError`] if any table cannot be read or the archive cannot be
/// written.
pub fn export<W: Write + Seek>(db: &Database, writer: W) -> Result<BackupStats, StorageError> {
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut zip = ZipWriter::new(writer);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let feed_folders = db.list_feed_folders()?;
    let feeds = db.list_feeds()?;
    let content_items = db.list_all_content_items()?;
    let tags = db.list_tags()?;
    let collections = db.list_collections()?;
    let feed_item_meta = db.list_all_feed_item_meta()?;
    let bookmark_meta = db.list_all_bookmark_meta()?;
    let highlight_meta = db.list_all_highlight_meta()?;
    let notes = db.list_all_notes()?;
    let review_cards = db.list_all_review_cards()?;
    let review_logs = db.list_all_review_logs()?;
    let rules = db.list_rules()?;
    let content_item_tags = db.list_all_content_item_tags()?;
    let collection_items = db.list_all_collection_items()?;

    let manifest = BackupManifest {
        app: MANIFEST_APP.to_owned(),
        schema_version: db.schema_version()?,
        created_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default(),
    };

    write_entry(&mut zip, &opts, entry::MANIFEST, &manifest)?;
    write_entry(&mut zip, &opts, entry::FEED_FOLDERS, &feed_folders)?;
    write_entry(&mut zip, &opts, entry::FEEDS, &feeds)?;
    write_entry(&mut zip, &opts, entry::CONTENT_ITEMS, &content_items)?;
    write_entry(&mut zip, &opts, entry::TAGS, &tags)?;
    write_entry(&mut zip, &opts, entry::COLLECTIONS, &collections)?;
    write_entry(&mut zip, &opts, entry::FEED_ITEM_META, &feed_item_meta)?;
    write_entry(&mut zip, &opts, entry::BOOKMARK_META, &bookmark_meta)?;
    write_entry(&mut zip, &opts, entry::HIGHLIGHT_META, &highlight_meta)?;
    write_entry(&mut zip, &opts, entry::NOTES, &notes)?;
    write_entry(&mut zip, &opts, entry::REVIEW_CARDS, &review_cards)?;
    write_entry(&mut zip, &opts, entry::REVIEW_LOGS, &review_logs)?;
    write_entry(&mut zip, &opts, entry::CONTENT_RULES, &rules)?;
    write_entry(
        &mut zip,
        &opts,
        entry::CONTENT_ITEM_TAGS,
        &content_item_tags,
    )?;
    write_entry(&mut zip, &opts, entry::COLLECTION_ITEMS, &collection_items)?;

    zip.finish()
        .map_err(|e| StorageError::Generic(format!("failed to finalize backup archive: {e}")))?;

    let total = feed_folders.len()
        + feeds.len()
        + content_items.len()
        + tags.len()
        + collections.len()
        + feed_item_meta.len()
        + bookmark_meta.len()
        + highlight_meta.len()
        + notes.len()
        + review_cards.len()
        + review_logs.len()
        + rules.len()
        + content_item_tags.len()
        + collection_items.len();

    Ok(BackupStats {
        feeds: feeds.len(),
        content_items: content_items.len(),
        tags: tags.len(),
        collections: collections.len(),
        notes: notes.len(),
        review_cards: review_cards.len(),
        rules: rules.len(),
        total,
    })
}

/// Restore a canonical backup archive from `reader` into `db`.
///
/// `db` must be empty (see [`Database::restore_backup`]); callers restoring over
/// an existing library should [`Database::reset`] first. The manifest is
/// validated: the archive must be a pergamon backup whose schema version is not
/// newer than `db`'s.
///
/// # Errors
///
/// Returns [`StorageError`] if the archive is not a valid pergamon backup, its
/// schema is newer than the database, or any entry cannot be parsed or inserted.
pub fn restore<R: Read + Seek>(db: &Database, reader: R) -> Result<BackupStats, StorageError> {
    use zip::ZipArchive;

    let mut archive = ZipArchive::new(reader)
        .map_err(|e| StorageError::Generic(format!("failed to read backup archive as ZIP: {e}")))?;

    let manifest: BackupManifest = read_entry(&mut archive, entry::MANIFEST)?;
    if manifest.app != MANIFEST_APP {
        return Err(StorageError::Generic(format!(
            "not a pergamon backup (manifest.app = {:?})",
            manifest.app
        )));
    }

    let current_version = db.schema_version()?;
    if manifest.schema_version > current_version {
        return Err(StorageError::Generic(format!(
            "backup schema version {} is newer than current {} — upgrade pergamon first",
            manifest.schema_version, current_version
        )));
    }

    let feed_folders: Vec<FeedFolder> = read_entry(&mut archive, entry::FEED_FOLDERS)?;
    let feeds: Vec<Feed> = read_entry(&mut archive, entry::FEEDS)?;
    let content_items: Vec<ContentItem> = read_entry(&mut archive, entry::CONTENT_ITEMS)?;
    let tags: Vec<Tag> = read_entry(&mut archive, entry::TAGS)?;
    let collections: Vec<Collection> = read_entry(&mut archive, entry::COLLECTIONS)?;
    let feed_item_meta: Vec<FeedItemMeta> = read_entry(&mut archive, entry::FEED_ITEM_META)?;
    let bookmark_meta: Vec<BookmarkMeta> = read_entry(&mut archive, entry::BOOKMARK_META)?;
    let highlight_meta: Vec<HighlightMeta> = read_entry(&mut archive, entry::HIGHLIGHT_META)?;
    let notes: Vec<Note> = read_entry_or_default(&mut archive, entry::NOTES)?;
    let review_cards: Vec<ReviewCard> = read_entry_or_default(&mut archive, entry::REVIEW_CARDS)?;
    let review_logs: Vec<ReviewLog> = read_entry_or_default(&mut archive, entry::REVIEW_LOGS)?;
    let rules: Vec<ContentRule> = read_entry_or_default(&mut archive, entry::CONTENT_RULES)?;
    let content_item_tags: Vec<(Uuid, Uuid)> = read_entry(&mut archive, entry::CONTENT_ITEM_TAGS)?;
    let collection_items: Vec<(Uuid, Uuid, i32)> =
        read_entry(&mut archive, entry::COLLECTION_ITEMS)?;

    db.restore_backup(
        &feed_folders,
        &feeds,
        &content_items,
        &tags,
        &collections,
        &feed_item_meta,
        &bookmark_meta,
        &highlight_meta,
        &content_item_tags,
        &collection_items,
        &notes,
        &review_cards,
        &review_logs,
        &rules,
    )?;

    let total = feed_folders.len()
        + feeds.len()
        + content_items.len()
        + tags.len()
        + collections.len()
        + feed_item_meta.len()
        + bookmark_meta.len()
        + highlight_meta.len()
        + notes.len()
        + review_cards.len()
        + review_logs.len()
        + rules.len()
        + content_item_tags.len()
        + collection_items.len();

    Ok(BackupStats {
        feeds: feeds.len(),
        content_items: content_items.len(),
        tags: tags.len(),
        collections: collections.len(),
        notes: notes.len(),
        review_cards: review_cards.len(),
        rules: rules.len(),
        total,
    })
}

/// Write one pretty-printed JSON entry to the archive.
fn write_entry<W: Write + Seek, T: Serialize>(
    zip: &mut zip::ZipWriter<W>,
    opts: &zip::write::SimpleFileOptions,
    name: &str,
    data: &T,
) -> Result<(), StorageError> {
    zip.start_file(name, *opts)
        .map_err(|e| StorageError::Generic(format!("failed to start ZIP entry {name}: {e}")))?;
    serde_json::to_writer_pretty(&mut *zip, data)
        .map_err(|e| StorageError::Generic(format!("failed to write JSON entry {name}: {e}")))?;
    Ok(())
}

/// Read and parse a required JSON entry from the archive.
fn read_entry<R: Read + Seek, T: for<'de> Deserialize<'de>>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<T, StorageError> {
    let entry = archive
        .by_name(name)
        .map_err(|e| StorageError::Generic(format!("missing backup entry {name}: {e}")))?;
    serde_json::from_reader(entry)
        .map_err(|e| StorageError::Generic(format!("failed to parse backup entry {name}: {e}")))
}

/// Read an optional JSON entry, returning the type default when it is absent.
///
/// Used for tables added after the earliest backup format so that older archives
/// (which predate the entry) still restore cleanly.
fn read_entry_or_default<R: Read + Seek, T: for<'de> Deserialize<'de> + Default>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<T, StorageError> {
    if archive.by_name(name).is_err() {
        return Ok(T::default());
    }
    read_entry(archive, name)
}
