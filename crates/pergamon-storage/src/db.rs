//! `SQLite` database wrapper with embedded migrations and CRUD operations.
//!
//! The [`Database`] struct owns a `rusqlite::Connection`, runs schema
//! migrations on open, and provides typed insert / query methods for every
//! entity in the unified content model.

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::diagnostics::{
    BrokenLinkRow, ContentTypeCount, ExtractionEvent, ExtractionSource, ExtractionStats,
    FeedHealthRow, FeedHealthStatus, ImportLogEntry, ImportSource, RuleMonitorRow, StatusCount,
    SystemStats,
};
use pergamon_core::fsrs::{CardState, Rating};
use pergamon_core::model::{
    BookmarkMeta, Collection, ContentItem, DailyReviewSummary, DailyUsageSummary, Feed, FeedFolder,
    FeedItemMeta, HighlightMeta, LinkHealth, MonthlyReviewSummary, MonthlyUsageSummary, Note,
    ReadingActivity, ReviewCard, ReviewLog, ReviewStats, ReviewStatsReport, SearchHit,
    SearchResult, SourceBreakdown, SourceRanking, Tag, TagCount, TagTrendPoint, UsageOverview,
    UsageStatsReport, WeeklyReviewSummary, WeeklyUsageSummary,
};
use pergamon_core::rule::{ContentRule, RuleAction};
use pergamon_core::status::DocumentStatus;

use crate::error::StorageError;

// ======================================================================
// Query filter
// ======================================================================

/// Sort order for content item listings.
///
/// Defaults to [`ContentItemSort::CreatedDesc`], matching the historical
/// behaviour of the listing queries (newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentItemSort {
    /// Most recently captured first (default).
    #[default]
    CreatedDesc,
    /// Alphabetical by title (case-insensitive).
    TitleAsc,
    /// Grouped by source feed title (case-insensitive), then newest first.
    SourceAsc,
}

/// Filter criteria for querying content items.
///
/// Combines multiple optional predicates. All specified predicates are
/// combined with AND. Feed and folder filters use JOINs through `feed_item_meta`.
/// Tag and collection filters use `EXISTS` subqueries.
#[derive(Debug, Clone, Default)]
pub struct ContentItemFilter {
    /// Filter by content type.
    pub content_type: Option<ContentType>,
    /// Filter by document status.
    pub status: Option<DocumentStatus>,
    /// Filter to items belonging to a specific feed.
    pub feed_id: Option<Uuid>,
    /// Filter to items belonging to feeds in a specific folder.
    pub folder_id: Option<Uuid>,
    /// Filter to items with a specific tag.
    pub tag_id: Option<Uuid>,
    /// Filter to items in a specific collection.
    pub collection_id: Option<Uuid>,
    /// Filter to items not in any collection.
    pub uncollected: bool,
    /// Sort order for the results.
    pub sort: ContentItemSort,
}

/// Filter criteria for full-text search queries.
///
/// All specified predicates are combined with AND alongside the FTS
/// `MATCH` clause. Search-specific facets (tag name, date range)
/// augment the base FTS query.
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    /// Filter by content type.
    pub content_type: Option<ContentType>,
    /// Filter by document status.
    pub status: Option<DocumentStatus>,
    /// Filter by tag name (case-insensitive match).
    pub tag_name: Option<String>,
    /// Filter to items belonging to a specific feed.
    pub feed_id: Option<Uuid>,
    /// Only include items created on or after this time.
    pub since: Option<OffsetDateTime>,
    /// Only include items created before this time.
    pub before: Option<OffsetDateTime>,
}

// ======================================================================
// Embedded migrations
// ======================================================================

/// Ordered list of migrations. Each entry is (version, description, sql).
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "initial_schema",
        include_str!("../migrations/V1__initial_schema.sql"),
    ),
    (
        2,
        "feed_health_tracking",
        include_str!("../migrations/V2__feed_health_tracking.sql"),
    ),
    (
        3,
        "feed_folders",
        include_str!("../migrations/V3__feed_folders.sql"),
    ),
    (
        4,
        "url_unique_index",
        include_str!("../migrations/V4__url_unique_index.sql"),
    ),
    (
        5,
        "bookmark_meta_enrichment",
        include_str!("../migrations/V5__bookmark_meta_enrichment.sql"),
    ),
    (
        6,
        "link_health",
        include_str!("../migrations/V6__link_health.sql"),
    ),
    (
        7,
        "notes_table",
        include_str!("../migrations/V7__notes_table.sql"),
    ),
    (
        8,
        "review_cards",
        include_str!("../migrations/V8__review_cards.sql"),
    ),
    (
        9,
        "smart_collections",
        include_str!("../migrations/V9__smart_collections.sql"),
    ),
    (
        10,
        "content_rules",
        include_str!("../migrations/V10__content_rules.sql"),
    ),
    (
        11,
        "read_at_column",
        include_str!("../migrations/V11__read_at_column.sql"),
    ),
    (
        12,
        "diagnostics_logs",
        include_str!("../migrations/V12__diagnostics_logs.sql"),
    ),
];

/// Run all pending migrations inside a transaction.
fn run_migrations(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS __schema_migrations (
            version     INTEGER PRIMARY KEY NOT NULL,
            description TEXT NOT NULL,
            applied_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );",
    )?;

    let applied_version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM __schema_migrations",
        [],
        |row| row.get(0),
    )?;

    for &(version, description, sql) in MIGRATIONS {
        if version > applied_version {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO __schema_migrations (version, description) VALUES (?1, ?2)",
                params![version, description],
            )?;
        }
    }

    Ok(())
}

/// `SQLite` database for pergamon content storage.
pub struct Database {
    conn: Connection,
}

#[allow(clippy::missing_errors_doc)]
impl Database {
    /// Open (or create) the database at `path` and run pending migrations.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established or migrations fail.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        // Enable WAL (Write-Ahead Logging) mode so concurrent readers do not
        // block a writer (and vice versa) — required for concurrent access from
        // the web server and CLI against the same database file (ADR-018).
        //
        // WAL creates two sidecar files alongside the main database that are
        // part of the live database state:
        //   * `<db>-wal` — the write-ahead log
        //   * `<db>-shm` — the shared-memory index
        // A raw file copy must include these (or stop all connections first);
        // application-level `export backup` is safe while running.
        //
        // `synchronous = NORMAL` is the recommended companion for WAL: durable
        // across application crashes with better performance than FULL.
        // `busy_timeout` waits on brief lock contention instead of returning
        // `SQLITE_BUSY` immediately. WAL requires a local filesystem with proper
        // locking (network filesystems such as NFS/SMB are unsupported).
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             PRAGMA busy_timeout = 5000;",
        )?;
        Self::init(&conn)?;
        Ok(Self { conn })
    }

    /// Create an in-memory database and run migrations. Useful for tests.
    ///
    /// # Errors
    ///
    /// Returns an error if migrations fail.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self { conn })
    }

    /// Shared initialisation: enable foreign keys and run migrations.
    fn init(conn: &Connection) -> Result<(), StorageError> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        run_migrations(conn)?;
        Ok(())
    }

    /// Run a closure inside a database transaction.
    ///
    /// If the closure returns `Ok`, the transaction is committed.
    /// If it returns `Err`, the transaction is rolled back.
    pub fn in_transaction<F, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Self) -> Result<T, E>,
        E: From<StorageError>,
    {
        self.conn
            .execute_batch("BEGIN;")
            .map_err(|e| E::from(StorageError::from(e)))?;
        match f(self) {
            Ok(val) => {
                self.conn
                    .execute_batch("COMMIT;")
                    .map_err(|e| E::from(StorageError::from(e)))?;
                Ok(val)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Begin a database transaction manually.
    ///
    /// Use with [`commit_transaction`](Self::commit_transaction) and
    /// [`rollback_transaction`](Self::rollback_transaction) when a closure-based
    /// transaction (via [`in_transaction`](Self::in_transaction)) isn't suitable
    /// (e.g. when the body mutates external state like caches or stats).
    pub fn begin_transaction(&self) -> Result<(), StorageError> {
        self.conn.execute_batch("BEGIN;")?;
        Ok(())
    }

    /// Commit a previously started transaction.
    pub fn commit_transaction(&self) -> Result<(), StorageError> {
        self.conn.execute_batch("COMMIT;")?;
        Ok(())
    }

    /// Roll back a previously started transaction.
    pub fn rollback_transaction(&self) -> Result<(), StorageError> {
        self.conn.execute_batch("ROLLBACK;")?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Feeds
    // ------------------------------------------------------------------

    /// Insert a new feed.
    pub fn insert_feed(&self, feed: &Feed) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO feeds (id, title, url, site_url, description, etag,
                last_modified_header, error_count, last_error,
                last_fetched_at, last_successful_fetch_at, folder_id,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                feed.id.to_string(),
                feed.title,
                feed.url,
                feed.site_url,
                feed.description,
                feed.etag,
                feed.last_modified_header,
                feed.error_count,
                feed.last_error,
                feed.last_fetched_at.map(fmt_time),
                feed.last_fetched_at.map(fmt_time), // last_successful_fetch_at = last_fetched_at on insert
                feed.folder_id.map(|id| id.to_string()),
                fmt_time(feed.created_at),
                fmt_time(feed.updated_at),
            ],
        )?;
        Ok(())
    }

    /// Retrieve a feed by ID.
    pub fn get_feed(&self, id: Uuid) -> Result<Feed, StorageError> {
        self.conn
            .query_row(
                "SELECT id, title, url, site_url, description, etag,
                        last_modified_header, error_count, last_error,
                        last_fetched_at, folder_id, created_at, updated_at
                 FROM feeds WHERE id = ?1",
                params![id.to_string()],
                |row| Ok(row_to_feed(row)),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "feed",
                id: id.to_string(),
            })
    }

    /// Retrieve a feed by its URL.
    pub fn get_feed_by_url(&self, url: &str) -> Result<Option<Feed>, StorageError> {
        self.conn
            .query_row(
                "SELECT id, title, url, site_url, description, etag,
                        last_modified_header, error_count, last_error,
                        last_fetched_at, folder_id, created_at, updated_at
                 FROM feeds WHERE url = ?1",
                params![url],
                |row| Ok(row_to_feed(row)),
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// List all feeds, ordered by title.
    pub fn list_feeds(&self) -> Result<Vec<Feed>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, url, site_url, description, etag,
                    last_modified_header, error_count, last_error,
                    last_fetched_at, folder_id, created_at, updated_at
             FROM feeds ORDER BY title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_feed(row)))?;
        let mut feeds = Vec::new();
        for row in rows {
            feeds.push(row?);
        }
        Ok(feeds)
    }

    /// Delete a feed and all associated data (cascades to `feed_item_meta`).
    pub fn delete_feed(&self, id: Uuid) -> Result<bool, StorageError> {
        let count = self
            .conn
            .execute("DELETE FROM feeds WHERE id = ?1", params![id.to_string()])?;
        Ok(count > 0)
    }

    /// Record a successful feed fetch (with or without new content).
    pub fn update_feed_fetch_success(
        &self,
        id: Uuid,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<(), StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());
        self.conn.execute(
            "UPDATE feeds SET
                etag = ?2,
                last_modified_header = ?3,
                error_count = 0,
                last_error = NULL,
                last_fetched_at = ?4,
                last_successful_fetch_at = ?4
             WHERE id = ?1",
            params![id.to_string(), etag, last_modified, now],
        )?;
        Ok(())
    }

    /// Record a failed feed fetch.
    pub fn update_feed_fetch_error(
        &self,
        id: Uuid,
        error_message: &str,
    ) -> Result<(), StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());
        self.conn.execute(
            "UPDATE feeds SET
                error_count = error_count + 1,
                last_error = ?2,
                last_fetched_at = ?3
             WHERE id = ?1",
            params![id.to_string(), error_message, now],
        )?;
        Ok(())
    }

    /// Check whether a feed item already exists by GUID within a feed.
    pub fn feed_item_exists_by_guid(
        &self,
        feed_id: Uuid,
        guid: &str,
    ) -> Result<bool, StorageError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM feed_item_meta
             WHERE feed_id = ?1 AND guid = ?2",
            params![feed_id.to_string(), guid],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Check whether a feed item already exists by URL within a feed.
    pub fn feed_item_exists_by_url(&self, feed_id: Uuid, url: &str) -> Result<bool, StorageError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM feed_item_meta fm
             JOIN content_items ci ON ci.id = fm.content_item_id
             WHERE fm.feed_id = ?1 AND ci.url = ?2",
            params![feed_id.to_string(), url],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Update the folder assignment for a feed.
    pub fn update_feed_folder_id(
        &self,
        feed_id: Uuid,
        folder_id: Option<Uuid>,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE feeds SET folder_id = ?1 WHERE id = ?2",
            params![folder_id.map(|id| id.to_string()), feed_id.to_string()],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Feed folders
    // ------------------------------------------------------------------

    /// Insert a new feed folder.
    pub fn insert_feed_folder(&self, folder: &FeedFolder) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO feed_folders (id, name, parent_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                folder.id.to_string(),
                folder.name,
                folder.parent_id.map(|id| id.to_string()),
                fmt_time(folder.created_at),
                fmt_time(folder.updated_at),
            ],
        )?;
        Ok(())
    }

    /// Find a feed folder by name within a parent (case-insensitive).
    pub fn get_feed_folder_by_name(
        &self,
        name: &str,
        parent_id: Option<Uuid>,
    ) -> Result<Option<FeedFolder>, StorageError> {
        let result = parent_id.map_or_else(
            || {
                self.conn.query_row(
                    "SELECT id, name, parent_id, created_at, updated_at
                     FROM feed_folders
                     WHERE name = ?1 COLLATE NOCASE AND parent_id IS NULL",
                    params![name],
                    |row| Ok(row_to_feed_folder(row)),
                )
            },
            |pid| {
                self.conn.query_row(
                    "SELECT id, name, parent_id, created_at, updated_at
                     FROM feed_folders
                     WHERE name = ?1 COLLATE NOCASE AND parent_id = ?2",
                    params![name, pid.to_string()],
                    |row| Ok(row_to_feed_folder(row)),
                )
            },
        );
        result.optional().map_err(StorageError::from)
    }

    /// List all feed folders, ordered by name.
    pub fn list_feed_folders(&self) -> Result<Vec<FeedFolder>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, parent_id, created_at, updated_at
             FROM feed_folders ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_feed_folder(row)))?;
        let mut folders = Vec::new();
        for row in rows {
            folders.push(row?);
        }
        Ok(folders)
    }

    /// Delete a feed folder. Returns true if the folder existed.
    pub fn delete_feed_folder(&self, id: Uuid) -> Result<bool, StorageError> {
        let count = self.conn.execute(
            "DELETE FROM feed_folders WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(count > 0)
    }

    // ------------------------------------------------------------------
    // Content items
    // ------------------------------------------------------------------

    /// Insert a new content item and update the FTS5 index.
    pub fn insert_content_item(&self, item: &ContentItem) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO content_items
                (id, url, title, author, content_type, status, content_text, excerpt, published_at, created_at, updated_at, read_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                item.id.to_string(),
                item.url,
                item.title,
                item.author,
                item.content_type.as_str(),
                item.status.as_str(),
                item.content_text,
                item.excerpt,
                item.published_at.map(fmt_time),
                fmt_time(item.created_at),
                fmt_time(item.updated_at),
                item.read_at.map(fmt_time),
            ],
        )?;

        self.upsert_fts(item)?;
        Ok(())
    }

    /// Retrieve a content item by ID.
    pub fn get_content_item(&self, id: Uuid) -> Result<ContentItem, StorageError> {
        self.conn
            .query_row(
                "SELECT id, url, title, author, content_type, status,
                        content_text, excerpt, published_at, created_at, updated_at, read_at
                 FROM content_items WHERE id = ?1",
                params![id.to_string()],
                |row| Ok(row_to_content_item(row)),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "content_item",
                id: id.to_string(),
            })
    }

    /// Find a content item by its URL.
    ///
    /// Returns `None` if no item has the given URL.
    pub fn get_content_item_by_url(&self, url: &str) -> Result<Option<ContentItem>, StorageError> {
        let item = self
            .conn
            .query_row(
                "SELECT id, url, title, author, content_type, status,
                        content_text, excerpt, published_at, created_at, updated_at, read_at
                 FROM content_items WHERE url = ?1",
                params![url],
                |row| Ok(row_to_content_item(row)),
            )
            .optional()?;
        Ok(item)
    }

    /// List content items matching a content type and/or status filter.
    ///
    /// Results are ordered by `created_at` descending. Use `limit` and
    /// `offset` for pagination.
    pub fn list_content_items(
        &self,
        content_type: Option<ContentType>,
        status: Option<DocumentStatus>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ContentItem>, StorageError> {
        let mut sql = String::from(
            "SELECT id, url, title, author, content_type, status,
                    content_text, excerpt, published_at, created_at, updated_at, read_at
             FROM content_items WHERE 1=1",
        );
        let mut param_values: Vec<String> = Vec::new();

        if let Some(ct) = content_type {
            param_values.push(ct.as_str().to_owned());
            let _ = write!(sql, " AND content_type = ?{}", param_values.len());
        }
        if let Some(st) = status {
            param_values.push(st.as_str().to_owned());
            let _ = write!(sql, " AND status = ?{}", param_values.len());
        }
        sql.push_str(" ORDER BY created_at DESC");

        if let Some(lim) = limit {
            let _ = write!(sql, " LIMIT {lim}");
        }
        if let Some(off) = offset {
            let _ = write!(sql, " OFFSET {off}");
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| Ok(row_to_content_item(row)))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// Update the status of a content item.
    ///
    /// When transitioning to `Archived`, also sets `read_at` to the current time.
    pub fn update_content_item_status(
        &self,
        id: Uuid,
        status: DocumentStatus,
    ) -> Result<(), StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());
        let affected = if status == DocumentStatus::Archived {
            self.conn.execute(
                "UPDATE content_items SET status = ?1, updated_at = ?2, read_at = COALESCE(read_at, ?2) WHERE id = ?3",
                params![status.as_str(), now, id.to_string()],
            )?
        } else {
            self.conn.execute(
                "UPDATE content_items SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, id.to_string()],
            )?
        };
        if affected == 0 {
            return Err(StorageError::NotFound {
                entity: "content_item",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    /// Count content items matching a status filter.
    pub fn count_content_items(&self, status: Option<DocumentStatus>) -> Result<u64, StorageError> {
        let count: i64 = if let Some(st) = status {
            self.conn.query_row(
                "SELECT COUNT(*) FROM content_items WHERE status = ?1",
                params![st.as_str()],
                |row| row.get(0),
            )?
        } else {
            self.conn
                .query_row("SELECT COUNT(*) FROM content_items", [], |row| row.get(0))?
        };
        #[allow(clippy::cast_sign_loss)]
        Ok(count as u64)
    }

    /// List content items matching a [`ContentItemFilter`].
    ///
    /// Results are ordered by `created_at` descending. Supports pagination
    /// via `limit` and `offset`. Feed/folder filters use JOINs through
    /// `feed_item_meta`.
    #[allow(clippy::missing_errors_doc)]
    pub fn list_content_items_filtered(
        &self,
        filter: &ContentItemFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ContentItem>, StorageError> {
        let (sql, param_values) = build_content_item_query(
            "SELECT DISTINCT ci.id, ci.url, ci.title, ci.author, ci.content_type, ci.status,
                    ci.content_text, ci.excerpt, ci.published_at, ci.created_at, ci.updated_at, ci.read_at",
            filter,
            limit,
            offset,
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| Ok(row_to_content_item(row)))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// Count content items matching a [`ContentItemFilter`].
    #[allow(clippy::missing_errors_doc)]
    pub fn count_content_items_filtered(
        &self,
        filter: &ContentItemFilter,
    ) -> Result<u64, StorageError> {
        let (sql, param_values) =
            build_content_item_query("SELECT COUNT(DISTINCT ci.id)", filter, None, None);

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let count: i64 = self
            .conn
            .query_row(&sql, param_refs.as_slice(), |row| row.get(0))?;
        #[allow(clippy::cast_sign_loss)]
        Ok(count as u64)
    }

    /// Bulk update the status of content items matching a filter.
    ///
    /// Returns the number of rows affected.
    #[allow(clippy::missing_errors_doc)]
    pub fn bulk_update_status(
        &self,
        filter: &ContentItemFilter,
        new_status: DocumentStatus,
    ) -> Result<u64, StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());

        // Build a subquery to find matching IDs.
        let (subquery, param_values) =
            build_content_item_query("SELECT DISTINCT ci.id", filter, None, None);

        let sql = format!(
            "UPDATE content_items SET status = ?1, updated_at = ?2 WHERE id IN ({subquery})"
        );

        let mut all_params: Vec<String> = vec![new_status.as_str().to_owned(), now];
        all_params.extend(param_values);

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();

        // Re-index the subquery parameters (they start at ?3 instead of ?1).
        let sql = reindex_params(&sql, 2);

        let affected = self.conn.execute(&sql, param_refs.as_slice())?;
        #[allow(clippy::cast_sign_loss)]
        Ok(affected as u64)
    }

    // ------------------------------------------------------------------
    // Extension tables
    // ------------------------------------------------------------------

    /// Insert feed item metadata.
    pub fn insert_feed_item_meta(&self, meta: &FeedItemMeta) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO feed_item_meta (content_item_id, feed_id, guid, summary)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                meta.content_item_id.to_string(),
                meta.feed_id.to_string(),
                meta.guid,
                meta.summary,
            ],
        )?;
        Ok(())
    }

    /// Retrieve feed item metadata by content item ID.
    pub fn get_feed_item_meta(&self, content_item_id: Uuid) -> Result<FeedItemMeta, StorageError> {
        self.conn
            .query_row(
                "SELECT content_item_id, feed_id, guid, summary
                 FROM feed_item_meta WHERE content_item_id = ?1",
                params![content_item_id.to_string()],
                |row| {
                    Ok(FeedItemMeta {
                        content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                        feed_id: parse_uuid(&row.get::<_, String>(1)?),
                        guid: row.get(2)?,
                        summary: row.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "feed_item_meta",
                id: content_item_id.to_string(),
            })
    }

    /// Insert bookmark metadata.
    pub fn insert_bookmark_meta(&self, meta: &BookmarkMeta) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO bookmark_meta (content_item_id, original_url, saved_from, thumbnail_url, description, site_name, favicon_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                meta.content_item_id.to_string(),
                meta.original_url,
                meta.saved_from,
                meta.thumbnail_url,
                meta.description,
                meta.site_name,
                meta.favicon_url,
            ],
        )?;
        Ok(())
    }

    /// Retrieve bookmark metadata by content item ID.
    pub fn get_bookmark_meta(&self, content_item_id: Uuid) -> Result<BookmarkMeta, StorageError> {
        self.conn
            .query_row(
                "SELECT content_item_id, original_url, saved_from, thumbnail_url, description, site_name, favicon_url
                 FROM bookmark_meta WHERE content_item_id = ?1",
                params![content_item_id.to_string()],
                |row| {
                    Ok(BookmarkMeta {
                        content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                        original_url: row.get(1)?,
                        saved_from: row.get(2)?,
                        thumbnail_url: row.get(3)?,
                        description: row.get(4)?,
                        site_name: row.get(5)?,
                        favicon_url: row.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "bookmark_meta",
                id: content_item_id.to_string(),
            })
    }

    /// Insert highlight metadata.
    pub fn insert_highlight_meta(&self, meta: &HighlightMeta) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO highlight_meta (content_item_id, source_item_id, quote_text, note, position_start, position_end, color)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                meta.content_item_id.to_string(),
                meta.source_item_id.map(|id| id.to_string()),
                meta.quote_text,
                meta.note,
                meta.position_start,
                meta.position_end,
                meta.color,
            ],
        )?;
        Ok(())
    }

    /// Retrieve highlight metadata by content item ID.
    pub fn get_highlight_meta(&self, content_item_id: Uuid) -> Result<HighlightMeta, StorageError> {
        self.conn
            .query_row(
                "SELECT content_item_id, source_item_id, quote_text, note,
                        position_start, position_end, color
                 FROM highlight_meta WHERE content_item_id = ?1",
                params![content_item_id.to_string()],
                |row| {
                    Ok(HighlightMeta {
                        content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                        source_item_id: row.get::<_, Option<String>>(1)?.map(|s| parse_uuid(&s)),
                        quote_text: row.get(2)?,
                        note: row.get(3)?,
                        position_start: row.get(4)?,
                        position_end: row.get(5)?,
                        color: row.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "highlight_meta",
                id: content_item_id.to_string(),
            })
    }

    /// Update the note field on an existing highlight.
    pub fn update_highlight_note(
        &self,
        content_item_id: Uuid,
        note: Option<&str>,
    ) -> Result<(), StorageError> {
        let affected = self.conn.execute(
            "UPDATE highlight_meta SET note = ?1 WHERE content_item_id = ?2",
            params![note, content_item_id.to_string()],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound {
                entity: "highlight_meta",
                id: content_item_id.to_string(),
            });
        }
        Ok(())
    }

    /// Update the note and color fields on an existing highlight.
    ///
    /// Both fields are set to the provided values (which may be `None` to
    /// clear them). Callers wanting PATCH semantics should read the existing
    /// [`HighlightMeta`] first and merge in only the changed fields.
    pub fn update_highlight_meta(
        &self,
        content_item_id: Uuid,
        note: Option<&str>,
        color: Option<&str>,
    ) -> Result<(), StorageError> {
        let affected = self.conn.execute(
            "UPDATE highlight_meta SET note = ?1, color = ?2 WHERE content_item_id = ?3",
            params![note, color, content_item_id.to_string()],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound {
                entity: "highlight_meta",
                id: content_item_id.to_string(),
            });
        }
        Ok(())
    }

    /// List highlights with optional filters.
    ///
    /// Returns `(ContentItem, HighlightMeta)` pairs sorted by creation date
    /// (newest first). Supports filtering by source item, tag name, and date
    /// range.
    pub fn list_highlights(
        &self,
        source_item_id: Option<Uuid>,
        tag_name: Option<&str>,
        since: Option<OffsetDateTime>,
        before: Option<OffsetDateTime>,
        limit: Option<u32>,
    ) -> Result<Vec<(ContentItem, HighlightMeta)>, StorageError> {
        let mut sql = String::from(
            "SELECT ci.id, ci.url, ci.title, ci.author, ci.content_type, ci.status,
                    ci.content_text, ci.excerpt, ci.published_at, ci.created_at, ci.updated_at, ci.read_at,
                    hm.content_item_id, hm.source_item_id, hm.quote_text, hm.note,
                    hm.position_start, hm.position_end, hm.color
             FROM content_items ci
             JOIN highlight_meta hm ON hm.content_item_id = ci.id",
        );
        let mut param_values: Vec<String> = Vec::new();

        if tag_name.is_some() {
            sql.push_str(
                " JOIN content_item_tags cit ON cit.content_item_id = ci.id
                  JOIN tags t ON t.id = cit.tag_id",
            );
        }

        sql.push_str(" WHERE ci.content_type = 'highlight'");

        if let Some(sid) = source_item_id {
            param_values.push(sid.to_string());
            let _ = write!(sql, " AND hm.source_item_id = ?{}", param_values.len());
        }
        if let Some(tag) = tag_name {
            param_values.push(tag.to_owned());
            let _ = write!(sql, " AND t.name = ?{} COLLATE NOCASE", param_values.len());
        }
        if let Some(s) = since {
            param_values.push(fmt_time(s));
            let _ = write!(sql, " AND ci.created_at >= ?{}", param_values.len());
        }
        if let Some(b) = before {
            param_values.push(fmt_time(b));
            let _ = write!(sql, " AND ci.created_at < ?{}", param_values.len());
        }

        sql.push_str(" ORDER BY ci.created_at DESC");

        if let Some(lim) = limit {
            let _ = write!(sql, " LIMIT {lim}");
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let item = row_to_content_item(row);
            let meta = HighlightMeta {
                content_item_id: parse_uuid(&row.get::<_, String>(12)?),
                source_item_id: row.get::<_, Option<String>>(13)?.map(|s| parse_uuid(&s)),
                quote_text: row.get(14)?,
                note: row.get(15)?,
                position_start: row.get(16)?,
                position_end: row.get(17)?,
                color: row.get(18)?,
            };
            Ok((item, meta))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Create a highlight from a source content item.
    ///
    /// Creates both the `content_items` row (type `highlight`) and the
    /// `highlight_meta` extension row. The `quote_text` is also stored as
    /// `content_text` on the content item so it participates in FTS5 search.
    ///
    /// If the quote text is found exactly once in the source item's
    /// `content_text`, the byte offsets are automatically recorded.
    pub fn create_highlight(
        &self,
        source_item_id: Uuid,
        quote_text: &str,
        note: Option<&str>,
        color: Option<&str>,
    ) -> Result<ContentItem, StorageError> {
        let source = self.get_content_item(source_item_id)?;

        // Auto-detect position offsets if quote appears exactly once.
        let (pos_start, pos_end) = source
            .content_text
            .as_deref()
            .and_then(|text| {
                let first = text.find(quote_text)?;
                // Check for a second occurrence.
                if text[first + quote_text.len()..].contains(quote_text) {
                    None // ambiguous
                } else {
                    Some((
                        Some(i64::try_from(first).ok()?),
                        Some(i64::try_from(first + quote_text.len()).ok()?),
                    ))
                }
            })
            .unwrap_or((None, None));

        let now = OffsetDateTime::now_utc();
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: None,
            title: truncate_for_title(quote_text),
            author: source.author,
            content_type: ContentType::Highlight,
            status: DocumentStatus::Inbox,
            content_text: Some(quote_text.to_owned()),
            excerpt: note.map(String::from),
            published_at: None,
            created_at: now,
            updated_at: now,
            read_at: None,
        };

        self.insert_content_item(&item)?;

        let meta = HighlightMeta {
            content_item_id: item.id,
            source_item_id: Some(source_item_id),
            quote_text: quote_text.to_owned(),
            note: note.map(String::from),
            position_start: pos_start,
            position_end: pos_end,
            color: color.map(String::from),
        };
        self.insert_highlight_meta(&meta)?;

        Ok(item)
    }

    // ------------------------------------------------------------------
    // Notes
    // ------------------------------------------------------------------

    /// Insert a new note.
    pub fn insert_note(&self, note: &Note) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO notes (id, content_item_id, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                note.id.to_string(),
                note.content_item_id.to_string(),
                note.body,
                fmt_time(note.created_at),
                fmt_time(note.updated_at),
            ],
        )?;
        Ok(())
    }

    /// Retrieve a note by ID.
    pub fn get_note(&self, id: Uuid) -> Result<Note, StorageError> {
        self.conn
            .query_row(
                "SELECT id, content_item_id, body, created_at, updated_at
                 FROM notes WHERE id = ?1",
                params![id.to_string()],
                |row| Ok(row_to_note(row)),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "note",
                id: id.to_string(),
            })
    }

    /// List all notes for a content item, ordered by creation date.
    pub fn list_notes_for_item(&self, content_item_id: Uuid) -> Result<Vec<Note>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_item_id, body, created_at, updated_at
             FROM notes WHERE content_item_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![content_item_id.to_string()], |row| {
            Ok(row_to_note(row))
        })?;
        let mut notes = Vec::new();
        for row in rows {
            notes.push(row?);
        }
        Ok(notes)
    }

    /// Update a note's body text.
    pub fn update_note(&self, id: Uuid, body: &str) -> Result<(), StorageError> {
        let affected = self.conn.execute(
            "UPDATE notes SET body = ?1 WHERE id = ?2",
            params![body, id.to_string()],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound {
                entity: "note",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    /// Delete a note by ID.
    pub fn delete_note(&self, id: Uuid) -> Result<bool, StorageError> {
        let affected = self
            .conn
            .execute("DELETE FROM notes WHERE id = ?1", params![id.to_string()])?;
        Ok(affected > 0)
    }

    /// List all notes (for backup/export).
    #[allow(clippy::missing_errors_doc)]
    pub fn list_all_notes(&self) -> Result<Vec<Note>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_item_id, body, created_at, updated_at
             FROM notes ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_note(row)))?;
        let mut notes = Vec::new();
        for row in rows {
            notes.push(row?);
        }
        Ok(notes)
    }

    // ------------------------------------------------------------------
    // Review cards (FSRS spaced repetition)
    // ------------------------------------------------------------------

    /// Insert a new review card for a highlight.
    pub fn insert_review_card(&self, card: &ReviewCard) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO review_cards (
                id, content_item_id, state, stability, difficulty,
                due_at, last_reviewed_at, review_count, lapse_count,
                scheduled_days, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                card.id.to_string(),
                card.content_item_id.to_string(),
                card.state.as_str(),
                card.stability,
                card.difficulty,
                fmt_time(card.due_at),
                card.last_reviewed_at.map(fmt_time),
                card.review_count,
                card.lapse_count,
                card.scheduled_days,
                fmt_time(card.created_at),
                fmt_time(card.updated_at),
            ],
        )?;
        Ok(())
    }

    /// Retrieve a review card by ID.
    pub fn get_review_card(&self, id: Uuid) -> Result<ReviewCard, StorageError> {
        self.conn
            .query_row(
                "SELECT id, content_item_id, state, stability, difficulty,
                        due_at, last_reviewed_at, review_count, lapse_count,
                        scheduled_days, created_at, updated_at
                 FROM review_cards WHERE id = ?1",
                params![id.to_string()],
                |row| Ok(row_to_review_card(row)),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "review_card",
                id: id.to_string(),
            })
    }

    /// Retrieve the review card for a given highlight content item.
    pub fn get_review_card_for_item(
        &self,
        content_item_id: Uuid,
    ) -> Result<Option<ReviewCard>, StorageError> {
        self.conn
            .query_row(
                "SELECT id, content_item_id, state, stability, difficulty,
                        due_at, last_reviewed_at, review_count, lapse_count,
                        scheduled_days, created_at, updated_at
                 FROM review_cards WHERE content_item_id = ?1",
                params![content_item_id.to_string()],
                |row| Ok(row_to_review_card(row)),
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// Update a review card after a review (state, memory, schedule).
    #[allow(clippy::too_many_arguments)]
    pub fn update_review_card(
        &self,
        id: Uuid,
        state: &str,
        stability: f64,
        difficulty: f64,
        due_at: OffsetDateTime,
        last_reviewed_at: OffsetDateTime,
        review_count: i32,
        lapse_count: i32,
        scheduled_days: f64,
    ) -> Result<(), StorageError> {
        let rows = self.conn.execute(
            "UPDATE review_cards SET
                state = ?1, stability = ?2, difficulty = ?3,
                due_at = ?4, last_reviewed_at = ?5,
                review_count = ?6, lapse_count = ?7, scheduled_days = ?8
             WHERE id = ?9",
            params![
                state,
                stability,
                difficulty,
                fmt_time(due_at),
                fmt_time(last_reviewed_at),
                review_count,
                lapse_count,
                scheduled_days,
                id.to_string(),
            ],
        )?;
        if rows == 0 {
            return Err(StorageError::NotFound {
                entity: "review_card",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    /// Delete a review card (disable review tracking for a highlight).
    pub fn delete_review_card(&self, id: Uuid) -> Result<bool, StorageError> {
        let rows = self.conn.execute(
            "DELETE FROM review_cards WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(rows > 0)
    }

    /// Delete the review card for a given highlight content item.
    pub fn delete_review_card_for_item(&self, content_item_id: Uuid) -> Result<bool, StorageError> {
        let rows = self.conn.execute(
            "DELETE FROM review_cards WHERE content_item_id = ?1",
            params![content_item_id.to_string()],
        )?;
        Ok(rows > 0)
    }

    /// List all review cards due on or before the given time.
    pub fn list_due_review_cards(
        &self,
        now: OffsetDateTime,
    ) -> Result<Vec<ReviewCard>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_item_id, state, stability, difficulty,
                    due_at, last_reviewed_at, review_count, lapse_count,
                    scheduled_days, created_at, updated_at
             FROM review_cards
             WHERE due_at <= ?1
             ORDER BY due_at ASC",
        )?;
        let rows = stmt.query_map(params![fmt_time(now)], |row| Ok(row_to_review_card(row)))?;
        let mut cards = Vec::new();
        for row in rows {
            cards.push(row?);
        }
        Ok(cards)
    }

    /// List all review cards.
    pub fn list_all_review_cards(&self) -> Result<Vec<ReviewCard>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_item_id, state, stability, difficulty,
                    due_at, last_reviewed_at, review_count, lapse_count,
                    scheduled_days, created_at, updated_at
             FROM review_cards ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_review_card(row)))?;
        let mut cards = Vec::new();
        for row in rows {
            cards.push(row?);
        }
        Ok(cards)
    }

    // ------------------------------------------------------------------
    // Review logs
    // ------------------------------------------------------------------

    /// Insert a review log entry.
    pub fn insert_review_log(&self, log: &ReviewLog) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO review_logs (
                id, card_id, rating, state_before, stability_before,
                difficulty_before, state_after, stability_after,
                difficulty_after, elapsed_days, scheduled_days, reviewed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                log.id.to_string(),
                log.card_id.to_string(),
                log.rating.value(),
                log.state_before.as_str(),
                log.stability_before,
                log.difficulty_before,
                log.state_after.as_str(),
                log.stability_after,
                log.difficulty_after,
                log.elapsed_days,
                log.scheduled_days,
                fmt_time(log.reviewed_at),
            ],
        )?;
        Ok(())
    }

    /// List all review logs for a given card.
    pub fn list_review_logs_for_card(&self, card_id: Uuid) -> Result<Vec<ReviewLog>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, card_id, rating, state_before, stability_before,
                    difficulty_before, state_after, stability_after,
                    difficulty_after, elapsed_days, scheduled_days, reviewed_at
             FROM review_logs
             WHERE card_id = ?1
             ORDER BY reviewed_at ASC",
        )?;
        let rows = stmt.query_map(params![card_id.to_string()], |row| {
            Ok(row_to_review_log(row))
        })?;
        let mut logs = Vec::new();
        for row in rows {
            logs.push(row?);
        }
        Ok(logs)
    }

    /// List all review logs (for backup export).
    pub fn list_all_review_logs(&self) -> Result<Vec<ReviewLog>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, card_id, rating, state_before, stability_before,
                    difficulty_before, state_after, stability_after,
                    difficulty_after, elapsed_days, scheduled_days, reviewed_at
             FROM review_logs ORDER BY reviewed_at ASC",
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_review_log(row)))?;
        let mut logs = Vec::new();
        for row in rows {
            logs.push(row?);
        }
        Ok(logs)
    }

    /// Compute aggregated review statistics.
    pub fn review_stats(&self, now: OffsetDateTime) -> Result<ReviewStats, StorageError> {
        let total_cards: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM review_cards", [], |row| row.get(0))?;
        let due_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_cards WHERE due_at <= ?1",
            params![fmt_time(now)],
            |row| row.get(0),
        )?;
        let total_reviews: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM review_logs", [], |row| row.get(0))?;
        let success_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_logs WHERE rating >= 2",
            [],
            |row| row.get(0),
        )?;
        #[allow(clippy::cast_precision_loss)]
        let observed_retention = if total_reviews > 0 {
            success_count as f64 / total_reviews as f64
        } else {
            0.0
        };

        let new_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_cards WHERE state = 'new'",
            [],
            |row| row.get(0),
        )?;
        let learning_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_cards WHERE state = 'learning'",
            [],
            |row| row.get(0),
        )?;
        let review_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_cards WHERE state = 'review'",
            [],
            |row| row.get(0),
        )?;
        let relearning_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_cards WHERE state = 'relearning'",
            [],
            |row| row.get(0),
        )?;

        let today_str = fmt_time(now);
        let today_date = &today_str[..10]; // "YYYY-MM-DD"
        let reviews_today: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_logs WHERE date(reviewed_at) = ?1",
            params![today_date],
            |row| row.get(0),
        )?;

        let (current_streak, longest_streak) = self.compute_streaks(today_date)?;

        Ok(ReviewStats {
            total_cards,
            due_count,
            total_reviews,
            success_count,
            observed_retention,
            new_count,
            learning_count,
            review_count,
            relearning_count,
            reviews_today,
            current_streak,
            longest_streak,
        })
    }

    /// Compute current and longest review streaks from review logs.
    ///
    /// A streak is a run of consecutive calendar days (UTC) with at least
    /// one review. The current streak counts backwards from today; if there
    /// are no reviews today, the streak is counted from yesterday so that
    /// users who haven't reviewed yet today don't see their streak reset.
    fn compute_streaks(&self, today_date: &str) -> Result<(i64, i64), StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT date(reviewed_at) AS d
             FROM review_logs
             ORDER BY d DESC",
        )?;
        let dates: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(std::result::Result::ok)
            .collect();

        if dates.is_empty() {
            return Ok((0, 0));
        }

        // Current streak: walk backwards from today (or yesterday if no reviews today).
        let mut current_streak: i64 = 0;
        let expect_date = |offset: i64| -> String {
            // Simple date arithmetic: parse today, subtract days.
            let year: i32 = today_date[..4].parse().unwrap_or(2026);
            let month: u32 = today_date[5..7].parse().unwrap_or(1);
            let day: u32 = today_date[8..10].parse().unwrap_or(1);

            // Convert to a simple day count and back (Julian day approximation).
            #[allow(clippy::cast_possible_truncation)]
            let jd = julian_day(year, month, day) - offset as i32;
            let (y, m, d) = from_julian_day(jd);
            format!("{y:04}-{m:02}-{d:02}")
        };

        // Determine where to start: today or yesterday.
        let start_offset: i64 = if dates.first().is_some_and(|d| d == today_date) {
            0
        } else if dates.first().is_some_and(|d| d == &expect_date(1)) {
            1
        } else {
            // Last review was more than a day ago — no active streak.
            // Still need to compute longest streak below.
            let longest = compute_longest_streak(&dates);
            return Ok((0, longest));
        };

        for i in 0.. {
            let expected = expect_date(start_offset + i);
            if dates.iter().any(|d| d == &expected) {
                current_streak += 1;
            } else {
                break;
            }
        }

        let longest = compute_longest_streak(&dates).max(current_streak);
        Ok((current_streak, longest))
    }

    /// Source breakdown: count review cards by provenance origin.
    ///
    /// Provenance is inferred from the source item's URL scheme:
    /// - `kindle://` → "Kindle"
    /// - `readwise://` → "Readwise"
    /// - Items with a feed subscription → "Feed"
    /// - Everything else → "Manual"
    pub fn review_source_breakdown(&self) -> Result<Vec<SourceBreakdown>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                CASE
                    WHEN ci.url LIKE 'kindle://%' THEN 'Kindle'
                    WHEN ci.url LIKE 'readwise://%' THEN 'Readwise'
                    WHEN fi.content_item_id IS NOT NULL THEN 'Feed'
                    ELSE 'Manual'
                END AS origin,
                COUNT(*) AS cnt
             FROM review_cards rc
             JOIN highlight_meta hm ON hm.content_item_id = rc.content_item_id
             LEFT JOIN content_items ci ON ci.id = hm.source_item_id
             LEFT JOIN feed_item_meta fi ON fi.content_item_id = hm.source_item_id
             GROUP BY origin
             ORDER BY cnt DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SourceBreakdown {
                origin: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Daily review activity for the last `days` calendar days.
    pub fn review_daily_history(
        &self,
        days: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<DailyReviewSummary>, StorageError> {
        let cutoff = now - time::Duration::days(days);
        let mut stmt = self.conn.prepare(
            "SELECT date(reviewed_at) AS d,
                    COUNT(*) AS total,
                    SUM(CASE WHEN rating >= 2 THEN 1 ELSE 0 END) AS good
             FROM review_logs
             WHERE reviewed_at >= ?1
             GROUP BY d
             ORDER BY d ASC",
        )?;
        let rows = stmt.query_map(params![fmt_time(cutoff)], |row| {
            Ok(DailyReviewSummary {
                date: row.get(0)?,
                reviews: row.get(1)?,
                successes: row.get(2)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Weekly review activity for the last `weeks` calendar weeks.
    pub fn review_weekly_history(
        &self,
        weeks: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<WeeklyReviewSummary>, StorageError> {
        let cutoff = now - time::Duration::days(weeks * 7);
        let mut stmt = self.conn.prepare(
            "SELECT strftime('%Y-W%W', reviewed_at) AS w,
                    COUNT(*) AS total,
                    SUM(CASE WHEN rating >= 2 THEN 1 ELSE 0 END) AS good
             FROM review_logs
             WHERE reviewed_at >= ?1
             GROUP BY w
             ORDER BY w ASC",
        )?;
        let rows = stmt.query_map(params![fmt_time(cutoff)], |row| {
            Ok(WeeklyReviewSummary {
                week: row.get(0)?,
                reviews: row.get(1)?,
                successes: row.get(2)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Monthly review activity for the last `months` calendar months.
    pub fn review_monthly_history(
        &self,
        months: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<MonthlyReviewSummary>, StorageError> {
        let cutoff = now - time::Duration::days(months * 30);
        let mut stmt = self.conn.prepare(
            "SELECT strftime('%Y-%m', reviewed_at) AS m,
                    COUNT(*) AS total,
                    SUM(CASE WHEN rating >= 2 THEN 1 ELSE 0 END) AS good
             FROM review_logs
             WHERE reviewed_at >= ?1
             GROUP BY m
             ORDER BY m ASC",
        )?;
        let rows = stmt.query_map(params![fmt_time(cutoff)], |row| {
            Ok(MonthlyReviewSummary {
                month: row.get(0)?,
                reviews: row.get(1)?,
                successes: row.get(2)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Build a full review stats report with all dimensions.
    pub fn review_stats_report(
        &self,
        now: OffsetDateTime,
    ) -> Result<ReviewStatsReport, StorageError> {
        let stats = self.review_stats(now)?;
        let source_breakdown = self.review_source_breakdown()?;
        let daily_history = self.review_daily_history(30, now)?;
        let weekly_history = self.review_weekly_history(12, now)?;
        let monthly_history = self.review_monthly_history(12, now)?;
        Ok(ReviewStatsReport {
            stats,
            source_breakdown,
            daily_history,
            weekly_history,
            monthly_history,
        })
    }

    // ------------------------------------------------------------------
    // Usage statistics
    // ------------------------------------------------------------------

    /// Build a complete usage statistics report.
    pub fn usage_stats_report(
        &self,
        now: OffsetDateTime,
    ) -> Result<UsageStatsReport, StorageError> {
        let overview = self.usage_overview(now)?;
        let daily = self.usage_daily(30, now)?;
        let weekly = self.usage_weekly(12, now)?;
        let monthly = self.usage_monthly(12, now)?;
        let top_sources = self.usage_top_sources(15)?;
        let tag_distribution = self.usage_tag_distribution(20)?;
        let tag_trends = self.usage_tag_trends(6)?;
        Ok(UsageStatsReport {
            overview,
            reading_activity: ReadingActivity {
                daily,
                weekly,
                monthly,
            },
            top_sources,
            tag_distribution,
            tag_trends,
        })
    }

    #[allow(clippy::cast_sign_loss)] // COUNT(*) is always non-negative
    fn usage_overview(&self, now: OffsetDateTime) -> Result<UsageOverview, StorageError> {
        let total_items: u64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM content_items", [], |r| {
                    r.get::<_, i64>(0)
                })? as u64;
        let inbox_count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM content_items WHERE status = 'inbox'",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64;
        let archived_count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM content_items WHERE status = 'archived'",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64;
        let total_highlights: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM content_items WHERE content_type = 'highlight'",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64;
        let total_feeds: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get::<_, i64>(0))?
            as u64;

        let now_str = fmt_time(now);
        let today_start = &now_str[..10];

        let items_saved_today: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM content_items \
             WHERE created_at >= ?1 || 'T00:00:00Z'",
            params![today_start],
            |r| r.get::<_, i64>(0),
        )? as u64;

        let week_ago = now - time::Duration::days(7);
        let week_str = fmt_time(week_ago);
        let items_saved_this_week: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM content_items WHERE created_at >= ?1",
            params![week_str],
            |r| r.get::<_, i64>(0),
        )? as u64;

        let month_ago = now - time::Duration::days(30);
        let month_str = fmt_time(month_ago);
        let items_saved_this_month: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM content_items WHERE created_at >= ?1",
            params![month_str],
            |r| r.get::<_, i64>(0),
        )? as u64;

        #[allow(clippy::cast_precision_loss)]
        let saves_per_day_30d = items_saved_this_month as f64 / 30.0;

        let highlight_rate = if total_items > 0 {
            #[allow(clippy::cast_precision_loss)]
            let rate = total_highlights as f64 / total_items as f64;
            rate
        } else {
            0.0
        };

        // Total reading time from archived readable items.
        let total_reading_minutes: u64 = self.conn.query_row(
            "SELECT COALESCE(SUM(LENGTH(content_text) - LENGTH(REPLACE(content_text, ' ', '')) + 1), 0) \
             FROM content_items \
             WHERE status = 'archived' \
               AND content_type IN ('article', 'feed_item', 'pdf') \
               AND content_text IS NOT NULL AND content_text != ''",
            [],
            |r| {
                let total_words: i64 = r.get(0)?;
                #[allow(clippy::cast_sign_loss)]
                let mins = words_to_minutes(total_words) as u64;
                Ok(mins)
            },
        )?;

        // Reading streaks (consecutive days with archived items).
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT DATE(read_at) FROM content_items \
             WHERE read_at IS NOT NULL \
               AND content_type IN ('article', 'feed_item', 'pdf') \
             ORDER BY read_at",
        )?;
        let dates: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .filter_map(Result::ok)
            .collect();

        let today_date = &now_str[..10];
        let (reading_streak_days, longest_reading_streak) =
            compute_reading_streaks(&dates, today_date);

        Ok(UsageOverview {
            total_items,
            inbox_count,
            archived_count,
            total_highlights,
            total_feeds,
            items_saved_today,
            items_saved_this_week,
            items_saved_this_month,
            saves_per_day_30d,
            highlight_rate,
            total_reading_minutes,
            reading_streak_days,
            longest_reading_streak,
        })
    }

    fn usage_daily(
        &self,
        days: u32,
        now: OffsetDateTime,
    ) -> Result<Vec<DailyUsageSummary>, StorageError> {
        let start = now - time::Duration::days(i64::from(days));
        let start_str = fmt_time(start);

        let mut saved_map = std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT DATE(created_at) AS d, COUNT(*) \
                 FROM content_items \
                 WHERE created_at >= ?1 \
                 GROUP BY d",
            )?;
            let rows = stmt.query_map(params![start_str], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (date, count) = row?;
                saved_map.insert(date, count);
            }
        }

        let mut read_map = std::collections::HashMap::new();
        let mut read_minutes_map = std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT DATE(read_at) AS d, COUNT(*), \
                 COALESCE(SUM(CASE WHEN content_text IS NOT NULL AND content_text != '' \
                   THEN LENGTH(content_text) - LENGTH(REPLACE(content_text, ' ', '')) + 1 \
                   ELSE 0 END), 0) \
                 FROM content_items \
                 WHERE read_at IS NOT NULL AND read_at >= ?1 \
                   AND content_type IN ('article', 'feed_item', 'pdf') \
                 GROUP BY d",
            )?;
            let rows = stmt.query_map(params![start_str], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (date, count, words) = row?;
                read_map.insert(date.clone(), count);
                read_minutes_map.insert(date, words_to_minutes(words));
            }
        }

        let mut result = Vec::with_capacity(days as usize);
        for i in 0..i64::from(days) {
            let day = now - time::Duration::days(i64::from(days) - 1 - i);
            let date = fmt_time(day)[..10].to_owned();
            result.push(DailyUsageSummary {
                items_saved: *saved_map.get(&date).unwrap_or(&0),
                items_read: *read_map.get(&date).unwrap_or(&0),
                reading_minutes: *read_minutes_map.get(&date).unwrap_or(&0),
                date,
            });
        }
        Ok(result)
    }

    fn usage_weekly(
        &self,
        weeks: u32,
        now: OffsetDateTime,
    ) -> Result<Vec<WeeklyUsageSummary>, StorageError> {
        let start = now - time::Duration::weeks(i64::from(weeks));
        let start_str = fmt_time(start);
        let total_days = i64::from(weeks) * 7;

        let mut stmt = self.conn.prepare(
            "WITH weeks AS ( \
               SELECT DISTINCT STRFTIME('%Y-W%W', d) AS w FROM ( \
                 SELECT DATE(?1, '+' || n || ' days') AS d \
                 FROM (WITH RECURSIVE cnt(n) AS ( \
                   SELECT 0 UNION ALL SELECT n+1 FROM cnt WHERE n < ?2 \
                 ) SELECT n FROM cnt) \
               ) \
             ) \
             SELECT w.w, \
               COALESCE(s.cnt, 0), \
               COALESCE(r.cnt, 0), \
               COALESCE(r.wds, 0) \
             FROM weeks w \
             LEFT JOIN ( \
               SELECT STRFTIME('%Y-W%W', created_at) AS w, COUNT(*) AS cnt \
               FROM content_items WHERE created_at >= ?1 GROUP BY w \
             ) s ON s.w = w.w \
             LEFT JOIN ( \
               SELECT STRFTIME('%Y-W%W', read_at) AS w, COUNT(*) AS cnt, \
                 COALESCE(SUM(CASE WHEN content_text IS NOT NULL AND content_text != '' \
                   THEN LENGTH(content_text) - LENGTH(REPLACE(content_text, ' ', '')) + 1 \
                   ELSE 0 END), 0) AS wds \
               FROM content_items WHERE read_at IS NOT NULL AND read_at >= ?1 \
                 AND content_type IN ('article', 'feed_item', 'pdf') GROUP BY w \
             ) r ON r.w = w.w \
             ORDER BY w.w",
        )?;
        let rows = stmt.query_map(params![start_str, total_days], |r| {
            let words: i64 = r.get(3)?;
            Ok(WeeklyUsageSummary {
                week: r.get(0)?,
                items_saved: r.get(1)?,
                items_read: r.get(2)?,
                reading_minutes: words_to_minutes(words),
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    fn usage_monthly(
        &self,
        months: u32,
        now: OffsetDateTime,
    ) -> Result<Vec<MonthlyUsageSummary>, StorageError> {
        let start = now - time::Duration::days(i64::from(months) * 30);
        let start_str = fmt_time(start);

        let mut result = Vec::new();
        let mut stmt = self.conn.prepare(
            "WITH months AS ( \
               SELECT DISTINCT STRFTIME('%Y-%m', d) AS m FROM ( \
                 SELECT DATE(?1, '+' || n || ' days') AS d \
                 FROM (WITH RECURSIVE cnt(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM cnt WHERE n < ?2) SELECT n FROM cnt) \
               ) \
             ) \
             SELECT mo.m, \
               COALESCE(s.cnt, 0), \
               COALESCE(r.cnt, 0), \
               COALESCE(r.mins, 0) \
             FROM months mo \
             LEFT JOIN ( \
               SELECT STRFTIME('%Y-%m', created_at) AS m, COUNT(*) AS cnt \
               FROM content_items WHERE created_at >= ?1 GROUP BY m \
             ) s ON s.m = mo.m \
             LEFT JOIN ( \
               SELECT STRFTIME('%Y-%m', read_at) AS m, COUNT(*) AS cnt, \
                 COALESCE(SUM(CASE WHEN content_text IS NOT NULL AND content_text != '' \
                   THEN LENGTH(content_text) - LENGTH(REPLACE(content_text, ' ', '')) + 1 \
                   ELSE 0 END), 0) AS mins \
               FROM content_items WHERE read_at IS NOT NULL AND read_at >= ?1 \
                 AND content_type IN ('article', 'feed_item', 'pdf') GROUP BY m \
             ) r ON r.m = mo.m \
             ORDER BY mo.m",
        )?;
        let total_days = i64::from(months) * 31;
        let rows = stmt.query_map(params![start_str, total_days], |r| {
            let words: i64 = r.get(3)?;
            Ok(MonthlyUsageSummary {
                month: r.get(0)?,
                items_saved: r.get(1)?,
                items_read: r.get(2)?,
                reading_minutes: words_to_minutes(words),
            })
        })?;
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    fn usage_top_sources(&self, limit: u32) -> Result<Vec<SourceRanking>, StorageError> {
        let mut result = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT \
               COALESCE(f.title, \
                 CASE WHEN ci.url IS NOT NULL AND ci.url != '' \
                   THEN REPLACE(REPLACE(REPLACE(ci.url, 'https://', ''), 'http://', ''), 'www.', '') \
                   ELSE '(unknown)' END \
               ) AS source_name, \
               SUM(CASE WHEN ci.status = 'archived' THEN 1 ELSE 0 END) AS read_count, \
               COUNT(*) AS total \
             FROM content_items ci \
             LEFT JOIN feed_item_meta fim ON fim.content_item_id = ci.id \
             LEFT JOIN feeds f ON f.id = fim.feed_id \
             WHERE ci.content_type IN ('article', 'feed_item', 'pdf') \
             GROUP BY source_name \
             ORDER BY read_count DESC, total DESC \
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(SourceRanking {
                source_name: r.get(0)?,
                items_read: r.get(1)?,
                total_items: r.get(2)?,
            })
        })?;
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    fn usage_tag_distribution(&self, limit: u32) -> Result<Vec<TagCount>, StorageError> {
        let mut result = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT t.name, COUNT(*) AS cnt \
             FROM tags t \
             JOIN content_item_tags cit ON cit.tag_id = t.id \
             GROUP BY t.name \
             ORDER BY cnt DESC \
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(TagCount {
                tag_name: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    fn usage_tag_trends(&self, months: u32) -> Result<Vec<TagTrendPoint>, StorageError> {
        let mut result = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT STRFTIME('%Y-%m', ci.created_at) AS m, t.name, COUNT(*) AS cnt \
             FROM content_item_tags cit \
             JOIN content_items ci ON ci.id = cit.content_item_id \
             JOIN tags t ON t.id = cit.tag_id \
             WHERE ci.created_at >= DATE('now', '-' || ?1 || ' months') \
             GROUP BY m, t.name \
             ORDER BY m, cnt DESC",
        )?;
        let rows = stmt.query_map(params![months], |r| {
            Ok(TagTrendPoint {
                month: r.get(0)?,
                tag_name: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ------------------------------------------------------------------
    // Tags
    // ------------------------------------------------------------------

    /// Insert a new tag.
    pub fn insert_tag(&self, tag: &Tag) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![tag.id.to_string(), tag.name, fmt_time(tag.created_at)],
        )?;
        Ok(())
    }

    /// Retrieve a tag by ID.
    pub fn get_tag(&self, id: Uuid) -> Result<Tag, StorageError> {
        self.conn
            .query_row(
                "SELECT id, name, created_at FROM tags WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok(Tag {
                        id: parse_uuid(&row.get::<_, String>(0)?),
                        name: row.get(1)?,
                        created_at: parse_time(&row.get::<_, String>(2)?),
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "tag",
                id: id.to_string(),
            })
    }

    /// Get an existing tag by name (case-insensitive) or create one.
    ///
    /// Uses `INSERT OR IGNORE` to avoid races, then selects the row.
    pub fn get_or_create_tag(&self, name: &str) -> Result<Tag, StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());
        let id = Uuid::new_v4();

        self.conn.execute(
            "INSERT OR IGNORE INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id.to_string(), name, now],
        )?;

        self.conn
            .query_row(
                "SELECT id, name, created_at FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |row| {
                    Ok(Tag {
                        id: parse_uuid(&row.get::<_, String>(0)?),
                        name: row.get(1)?,
                        created_at: parse_time(&row.get::<_, String>(2)?),
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "tag",
                id: name.to_owned(),
            })
    }

    /// Associate a tag with a content item and refresh the FTS index.
    pub fn tag_content_item(
        &self,
        content_item_id: Uuid,
        tag_id: Uuid,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO content_item_tags (content_item_id, tag_id)
             VALUES (?1, ?2)",
            params![content_item_id.to_string(), tag_id.to_string()],
        )?;

        self.refresh_fts_tags(content_item_id)?;
        Ok(())
    }

    /// Remove a tag from a content item and refresh the FTS index.
    pub fn untag_content_item(
        &self,
        content_item_id: Uuid,
        tag_id: Uuid,
    ) -> Result<bool, StorageError> {
        let count = self.conn.execute(
            "DELETE FROM content_item_tags
             WHERE content_item_id = ?1 AND tag_id = ?2",
            params![content_item_id.to_string(), tag_id.to_string()],
        )?;
        if count > 0 {
            self.refresh_fts_tags(content_item_id)?;
        }
        Ok(count > 0)
    }

    /// Delete a tag and remove it from all items.
    ///
    /// Refreshes the FTS index for all previously tagged items.
    pub fn delete_tag(&self, id: Uuid) -> Result<bool, StorageError> {
        // Collect affected items before deleting.
        let affected_items: Vec<Uuid> = {
            let mut stmt = self
                .conn
                .prepare("SELECT content_item_id FROM content_item_tags WHERE tag_id = ?1")?;
            let rows = stmt.query_map(params![id.to_string()], |row| {
                Ok(parse_uuid(&row.get::<_, String>(0)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let count = self
            .conn
            .execute("DELETE FROM tags WHERE id = ?1", params![id.to_string()])?;

        // Refresh FTS for affected items.
        for item_id in &affected_items {
            self.refresh_fts_tags(*item_id)?;
        }

        Ok(count > 0)
    }

    /// Rename a tag and refresh FTS for all tagged items.
    pub fn rename_tag(&self, id: Uuid, new_name: &str) -> Result<(), StorageError> {
        let affected = self.conn.execute(
            "UPDATE tags SET name = ?1 WHERE id = ?2",
            params![new_name, id.to_string()],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound {
                entity: "tag",
                id: id.to_string(),
            });
        }

        // Refresh FTS for all items with this tag.
        let affected_items: Vec<Uuid> = {
            let mut stmt = self
                .conn
                .prepare("SELECT content_item_id FROM content_item_tags WHERE tag_id = ?1")?;
            let rows = stmt.query_map(params![id.to_string()], |row| {
                Ok(parse_uuid(&row.get::<_, String>(0)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for item_id in &affected_items {
            self.refresh_fts_tags(*item_id)?;
        }

        Ok(())
    }

    /// Merge one tag into another.
    ///
    /// Reassigns every item tagged with `from_id` to `to_id` (skipping items
    /// already carrying `to_id`), then deletes the now-empty `from` tag. FTS is
    /// refreshed for all affected items so tag-based search stays accurate. The
    /// whole operation runs in a transaction. A no-op when `from_id == to_id`.
    pub fn merge_tags(&self, from_id: Uuid, to_id: Uuid) -> Result<(), StorageError> {
        if from_id == to_id {
            return Ok(());
        }

        // Both tags must exist.
        self.get_tag(from_id)?;
        self.get_tag(to_id)?;

        // Capture affected items before mutating so FTS can be refreshed after.
        let affected_items: Vec<Uuid> = {
            let mut stmt = self
                .conn
                .prepare("SELECT content_item_id FROM content_item_tags WHERE tag_id = ?1")?;
            let rows = stmt.query_map(params![from_id.to_string()], |row| {
                Ok(parse_uuid(&row.get::<_, String>(0)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        self.conn.execute_batch("BEGIN;")?;
        let result = (|| {
            // Copy memberships to the target tag, ignoring rows that already exist.
            self.conn.execute(
                "INSERT OR IGNORE INTO content_item_tags (content_item_id, tag_id)
                 SELECT content_item_id, ?1 FROM content_item_tags WHERE tag_id = ?2",
                params![to_id.to_string(), from_id.to_string()],
            )?;
            // Deleting the source tag cascades its membership rows.
            self.conn.execute(
                "DELETE FROM tags WHERE id = ?1",
                params![from_id.to_string()],
            )?;
            Ok::<(), StorageError>(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT;")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                return Err(e);
            }
        }

        for item_id in &affected_items {
            self.refresh_fts_tags(*item_id)?;
        }

        Ok(())
    }

    /// Find a tag by name (case-insensitive).
    pub fn get_tag_by_name(&self, name: &str) -> Result<Option<Tag>, StorageError> {
        self.conn
            .query_row(
                "SELECT id, name, created_at FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |row| {
                    Ok(Tag {
                        id: parse_uuid(&row.get::<_, String>(0)?),
                        name: row.get(1)?,
                        created_at: parse_time(&row.get::<_, String>(2)?),
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// List tags for a specific content item.
    pub fn tags_for_item(&self, content_item_id: Uuid) -> Result<Vec<Tag>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name, t.created_at
             FROM tags t
             JOIN content_item_tags ct ON ct.tag_id = t.id
             WHERE ct.content_item_id = ?1
             ORDER BY t.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![content_item_id.to_string()], |row| {
            Ok(Tag {
                id: parse_uuid(&row.get::<_, String>(0)?),
                name: row.get(1)?,
                created_at: parse_time(&row.get::<_, String>(2)?),
            })
        })?;
        let mut tags = Vec::new();
        for row in rows {
            tags.push(row?);
        }
        Ok(tags)
    }

    /// List content items with a specific tag.
    pub fn list_items_by_tag(&self, tag_id: Uuid) -> Result<Vec<ContentItem>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT ci.id, ci.url, ci.title, ci.author, ci.content_type, ci.status,
                    ci.content_text, ci.excerpt, ci.published_at, ci.created_at, ci.updated_at, ci.read_at
             FROM content_items ci
             JOIN content_item_tags ct ON ct.content_item_id = ci.id
             WHERE ct.tag_id = ?1
             ORDER BY ci.created_at DESC",
        )?;
        let rows = stmt.query_map(params![tag_id.to_string()], |row| {
            Ok(row_to_content_item(row))
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    // ------------------------------------------------------------------
    // Collections
    // ------------------------------------------------------------------

    /// Insert a new collection.
    pub fn insert_collection(&self, coll: &Collection) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO collections (id, name, parent_id, sort_order, is_smart, filter_query, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                coll.id.to_string(),
                coll.name,
                coll.parent_id.map(|id| id.to_string()),
                coll.sort_order,
                i32::from(coll.is_smart),
                coll.filter_query,
                fmt_time(coll.created_at),
                fmt_time(coll.updated_at),
            ],
        )?;
        Ok(())
    }

    /// Retrieve a collection by ID.
    pub fn get_collection(&self, id: Uuid) -> Result<Collection, StorageError> {
        self.conn
            .query_row(
                "SELECT id, name, parent_id, sort_order, is_smart, filter_query, created_at, updated_at
                 FROM collections WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok(Collection {
                        id: parse_uuid(&row.get::<_, String>(0)?),
                        name: row.get(1)?,
                        parent_id: row.get::<_, Option<String>>(2)?.map(|s| parse_uuid(&s)),
                        sort_order: row.get(3)?,
                        is_smart: row.get::<_, i32>(4)? != 0,
                        filter_query: row.get(5)?,
                        created_at: parse_time(&row.get::<_, String>(6)?),
                        updated_at: parse_time(&row.get::<_, String>(7)?),
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                entity: "collection",
                id: id.to_string(),
            })
    }

    /// Add a content item to a collection.
    ///
    /// Returns an error if the collection is a smart collection (membership
    /// is computed dynamically).
    pub fn add_to_collection(
        &self,
        content_item_id: Uuid,
        collection_id: Uuid,
        sort_order: i32,
    ) -> Result<(), StorageError> {
        let coll = self.get_collection(collection_id)?;
        if coll.is_smart {
            return Err(StorageError::Constraint(
                "cannot manually add items to a smart collection".to_owned(),
            ));
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO content_item_collections (content_item_id, collection_id, sort_order)
             VALUES (?1, ?2, ?3)",
            params![
                content_item_id.to_string(),
                collection_id.to_string(),
                sort_order,
            ],
        )?;
        Ok(())
    }

    /// Remove a content item from a collection.
    ///
    /// Returns an error if the collection is a smart collection.
    pub fn remove_from_collection(
        &self,
        content_item_id: Uuid,
        collection_id: Uuid,
    ) -> Result<bool, StorageError> {
        let coll = self.get_collection(collection_id)?;
        if coll.is_smart {
            return Err(StorageError::Constraint(
                "cannot manually remove items from a smart collection".to_owned(),
            ));
        }
        let count = self.conn.execute(
            "DELETE FROM content_item_collections
             WHERE content_item_id = ?1 AND collection_id = ?2",
            params![content_item_id.to_string(), collection_id.to_string()],
        )?;
        Ok(count > 0)
    }

    /// Rename a collection.
    pub fn rename_collection(&self, id: Uuid, new_name: &str) -> Result<(), StorageError> {
        let affected = self.conn.execute(
            "UPDATE collections SET name = ?1 WHERE id = ?2",
            params![new_name, id.to_string()],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound {
                entity: "collection",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    /// Move a collection to a new parent.
    ///
    /// Rejects the move if it would create a cycle (moving a collection
    /// under itself or one of its descendants).
    pub fn move_collection(
        &self,
        id: Uuid,
        new_parent_id: Option<Uuid>,
    ) -> Result<(), StorageError> {
        if let Some(parent) = new_parent_id {
            if parent == id {
                return Err(StorageError::Generic(
                    "cannot move a collection under itself".into(),
                ));
            }
            // Walk ancestors of new_parent_id to detect cycles.
            let mut cursor = Some(parent);
            while let Some(cur) = cursor {
                let ancestor: Option<Option<String>> = self
                    .conn
                    .query_row(
                        "SELECT parent_id FROM collections WHERE id = ?1",
                        params![cur.to_string()],
                        |row| row.get(0),
                    )
                    .optional()?;
                match ancestor {
                    Some(Some(pid)) => {
                        let pid = parse_uuid(&pid);
                        if pid == id {
                            return Err(StorageError::Generic(
                                "cannot move a collection under one of its descendants".into(),
                            ));
                        }
                        cursor = Some(pid);
                    }
                    _ => cursor = None,
                }
            }
        }

        let affected = self.conn.execute(
            "UPDATE collections SET parent_id = ?1 WHERE id = ?2",
            params![new_parent_id.map(|p| p.to_string()), id.to_string()],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound {
                entity: "collection",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    /// Delete a collection. Returns true if the collection existed.
    ///
    /// Child collections are promoted (`parent_id` set to NULL) and item
    /// memberships are removed by the ON DELETE CASCADE foreign key.
    pub fn delete_collection(&self, id: Uuid) -> Result<bool, StorageError> {
        let count = self.conn.execute(
            "DELETE FROM collections WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(count > 0)
    }

    /// Find a collection by name (case-insensitive).
    pub fn get_collection_by_name(&self, name: &str) -> Result<Option<Collection>, StorageError> {
        self.conn
            .query_row(
                "SELECT id, name, parent_id, sort_order, is_smart, filter_query, created_at, updated_at
                 FROM collections WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |row| {
                    Ok(Collection {
                        id: parse_uuid(&row.get::<_, String>(0)?),
                        name: row.get(1)?,
                        parent_id: row.get::<_, Option<String>>(2)?.map(|s| parse_uuid(&s)),
                        sort_order: row.get(3)?,
                        is_smart: row.get::<_, i32>(4)? != 0,
                        filter_query: row.get(5)?,
                        created_at: parse_time(&row.get::<_, String>(6)?),
                        updated_at: parse_time(&row.get::<_, String>(7)?),
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// List content items in a specific collection.
    pub fn list_collection_items(
        &self,
        collection_id: Uuid,
    ) -> Result<Vec<ContentItem>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT ci.id, ci.url, ci.title, ci.author, ci.content_type, ci.status,
                    ci.content_text, ci.excerpt, ci.published_at, ci.created_at, ci.updated_at, ci.read_at
             FROM content_items ci
             JOIN content_item_collections cic ON cic.content_item_id = ci.id
             WHERE cic.collection_id = ?1
             ORDER BY cic.sort_order, ci.created_at DESC",
        )?;
        let rows = stmt.query_map(params![collection_id.to_string()], |row| {
            Ok(row_to_content_item(row))
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// Reorder the members of a (manual) collection.
    ///
    /// Assigns each item in `ordered_item_ids` a `sort_order` matching its
    /// position in the slice. Items belonging to the collection but absent from
    /// the slice are left untouched (their existing `sort_order` is preserved).
    /// Rejects smart collections, whose membership is computed, not stored.
    /// Runs in a transaction.
    pub fn reorder_collection_items(
        &self,
        collection_id: Uuid,
        ordered_item_ids: &[Uuid],
    ) -> Result<(), StorageError> {
        let coll = self.get_collection(collection_id)?;
        if coll.is_smart {
            return Err(StorageError::Constraint(
                "cannot reorder items in a smart collection".to_owned(),
            ));
        }

        self.conn.execute_batch("BEGIN;")?;
        let result = (|| {
            for (position, item_id) in ordered_item_ids.iter().enumerate() {
                let order = i64::try_from(position).unwrap_or(i64::MAX);
                self.conn.execute(
                    "UPDATE content_item_collections SET sort_order = ?1
                     WHERE collection_id = ?2 AND content_item_id = ?3",
                    params![order, collection_id.to_string(), item_id.to_string()],
                )?;
            }
            Ok::<(), StorageError>(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------------
    // Bulk listing (backup / export)
    // ------------------------------------------------------------------

    /// List all content items (no filter, no limit).
    #[allow(clippy::missing_errors_doc)]
    pub fn list_all_content_items(&self) -> Result<Vec<ContentItem>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, author, content_type, status,
                    content_text, excerpt, published_at, created_at, updated_at, read_at
             FROM content_items ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_content_item(row)))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// List all collections.
    #[allow(clippy::missing_errors_doc)]
    pub fn list_collections(&self) -> Result<Vec<Collection>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, parent_id, sort_order, is_smart, filter_query, created_at, updated_at
             FROM collections ORDER BY sort_order, name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Collection {
                id: parse_uuid(&row.get::<_, String>(0)?),
                name: row.get(1)?,
                parent_id: row.get::<_, Option<String>>(2)?.map(|s| parse_uuid(&s)),
                sort_order: row.get(3)?,
                is_smart: row.get::<_, i32>(4)? != 0,
                filter_query: row.get(5)?,
                created_at: parse_time(&row.get::<_, String>(6)?),
                updated_at: parse_time(&row.get::<_, String>(7)?),
            })
        })?;
        let mut colls = Vec::new();
        for row in rows {
            colls.push(row?);
        }
        Ok(colls)
    }

    /// List items matching a smart collection's filter.
    ///
    /// Parses the collection's `filter_query` and dynamically queries
    /// matching content items. Returns an error if the collection is not smart.
    pub fn list_smart_collection_items(
        &self,
        collection_id: Uuid,
    ) -> Result<Vec<ContentItem>, StorageError> {
        let coll = self.get_collection(collection_id)?;
        if !coll.is_smart {
            return Err(StorageError::Constraint(
                "not a smart collection".to_owned(),
            ));
        }
        let filter_str = coll.filter_query.as_deref().unwrap_or("");
        let smart = pergamon_core::smart_filter::SmartFilter::parse(filter_str)
            .map_err(|e| StorageError::Generic(format!("invalid smart filter: {e}")))?;

        let (sql, param_values) = build_smart_filter_query(&smart, None);
        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| Ok(row_to_content_item(row)))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// Count items matching a smart collection's filter.
    pub fn count_smart_collection_items(&self, collection_id: Uuid) -> Result<usize, StorageError> {
        let coll = self.get_collection(collection_id)?;
        if !coll.is_smart {
            return Err(StorageError::Constraint(
                "not a smart collection".to_owned(),
            ));
        }
        let filter_str = coll.filter_query.as_deref().unwrap_or("");
        let smart = pergamon_core::smart_filter::SmartFilter::parse(filter_str)
            .map_err(|e| StorageError::Generic(format!("invalid smart filter: {e}")))?;

        let (sql, param_values) = build_smart_filter_count(&smart);
        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let count: i64 = stmt.query_row(param_refs.as_slice(), |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Update the filter query of a smart collection.
    pub fn update_smart_filter(
        &self,
        collection_id: Uuid,
        filter_query: &str,
    ) -> Result<(), StorageError> {
        // Validate the filter parses correctly before storing.
        pergamon_core::smart_filter::SmartFilter::parse(filter_query)
            .map_err(|e| StorageError::Generic(format!("invalid smart filter: {e}")))?;

        self.conn.execute(
            "UPDATE collections SET filter_query = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?2 AND is_smart = 1",
            params![filter_query, collection_id.to_string()],
        )?;
        Ok(())
    }

    // ==================================================================
    // Content rules
    // ==================================================================

    /// Insert a new content rule.
    pub fn insert_rule(&self, rule: &ContentRule) -> Result<(), StorageError> {
        let actions_json = serde_json::to_string(&rule.actions)
            .map_err(|e| StorageError::Generic(format!("failed to serialize actions: {e}")))?;
        self.conn.execute(
            "INSERT INTO content_rules (id, name, enabled, priority, filter_query, actions_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                rule.id.to_string(),
                rule.name,
                i32::from(rule.enabled),
                rule.priority,
                rule.filter_query,
                actions_json,
                fmt_time(rule.created_at),
                fmt_time(rule.updated_at),
            ],
        )?;
        Ok(())
    }

    /// Get a content rule by ID.
    pub fn get_rule(&self, id: Uuid) -> Result<ContentRule, StorageError> {
        self.conn
            .query_row(
                "SELECT id, name, enabled, priority, filter_query, actions_json, created_at, updated_at
                 FROM content_rules WHERE id = ?1",
                params![id.to_string()],
                |row| Ok(row_to_rule(row)),
            )
            .optional()?
            .ok_or(StorageError::NotFound {
                entity: "content_rule",
                id: id.to_string(),
            })
    }

    /// Get a content rule by name (case-insensitive).
    pub fn get_rule_by_name(&self, name: &str) -> Result<Option<ContentRule>, StorageError> {
        self.conn
            .query_row(
                "SELECT id, name, enabled, priority, filter_query, actions_json, created_at, updated_at
                 FROM content_rules WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |row| Ok(row_to_rule(row)),
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// List all content rules, ordered by priority then name.
    pub fn list_rules(&self) -> Result<Vec<ContentRule>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, enabled, priority, filter_query, actions_json, created_at, updated_at
             FROM content_rules ORDER BY priority, name",
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_rule(row)))?;
        let mut rules = Vec::new();
        for row in rows {
            rules.push(row?);
        }
        Ok(rules)
    }

    /// Delete a content rule by ID. Returns true if a row was deleted.
    pub fn delete_rule(&self, id: Uuid) -> Result<bool, StorageError> {
        let count = self.conn.execute(
            "DELETE FROM content_rules WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(count > 0)
    }

    /// Enable or disable a content rule.
    pub fn set_rule_enabled(&self, id: Uuid, enabled: bool) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE content_rules SET enabled = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?2",
            params![i32::from(enabled), id.to_string()],
        )?;
        Ok(())
    }

    // ==================================================================
    // Diagnostics: import history
    // ==================================================================

    /// Record an import run in the import history log.
    pub fn insert_import_log(&self, entry: &ImportLogEntry) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO import_log
                (id, source, file_name, items_added, items_existing, items_skipped,
                 errors, error_detail, dry_run, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.id.to_string(),
                entry.source.as_str(),
                entry.file_name,
                entry.items_added,
                entry.items_existing,
                entry.items_skipped,
                entry.errors,
                entry.error_detail,
                i32::from(entry.dry_run),
                fmt_time(entry.created_at),
            ],
        )?;
        Ok(())
    }

    /// List the most recent import runs, newest first.
    pub fn list_import_logs(&self, limit: u32) -> Result<Vec<ImportLogEntry>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, file_name, items_added, items_existing, items_skipped,
                    errors, error_detail, dry_run, created_at
             FROM import_log ORDER BY created_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| Ok(row_to_import_log(row)))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    // ==================================================================
    // Diagnostics: extraction events
    // ==================================================================

    /// Record a content-extraction attempt.
    pub fn insert_extraction_event(&self, event: &ExtractionEvent) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO extraction_log
                (id, content_item_id, url, source, success, extractor, error_message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.id.to_string(),
                event.content_item_id.map(|id| id.to_string()),
                event.url,
                event.source.as_str(),
                i32::from(event.success),
                event.extractor,
                event.error_message,
                fmt_time(event.created_at),
            ],
        )?;
        Ok(())
    }

    /// List the most recent extraction events, newest first.
    ///
    /// When `only_failures` is true, only failed attempts are returned.
    pub fn list_extraction_events(
        &self,
        limit: u32,
        only_failures: bool,
    ) -> Result<Vec<ExtractionEvent>, StorageError> {
        let sql = if only_failures {
            "SELECT id, content_item_id, url, source, success, extractor, error_message, created_at
             FROM extraction_log WHERE success = 0
             ORDER BY created_at DESC, id DESC LIMIT ?1"
        } else {
            "SELECT id, content_item_id, url, source, success, extractor, error_message, created_at
             FROM extraction_log ORDER BY created_at DESC, id DESC LIMIT ?1"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![limit], |row| Ok(row_to_extraction_event(row)))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    /// Aggregate extraction success / failure statistics over the whole log.
    pub fn extraction_stats(&self) -> Result<ExtractionStats, StorageError> {
        let (succeeded, failed): (i64, i64) = self.conn.query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END), 0)
             FROM extraction_log",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(ExtractionStats::new(succeeded, failed))
    }

    // ==================================================================
    // Diagnostics: feed health
    // ==================================================================

    /// Build a feed-health report classifying each feed and flagging stale ones.
    ///
    /// A feed is stale when its last successful fetch is older than
    /// `stale_after_days` (or it has never been fetched).
    pub fn feed_health(
        &self,
        stale_after_days: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<FeedHealthRow>, StorageError> {
        let feeds = self.list_feeds()?;
        let stale_cutoff = now - time::Duration::days(stale_after_days);
        let rows = feeds
            .into_iter()
            .map(|f| {
                let is_stale = f.last_fetched_at.is_none_or(|t| t < stale_cutoff);
                FeedHealthRow {
                    feed_id: f.id,
                    title: f.title,
                    url: f.url,
                    status: FeedHealthStatus::from_error_count(f.error_count),
                    error_count: f.error_count,
                    last_error: f.last_error,
                    last_fetched_at: f.last_fetched_at,
                    is_stale,
                }
            })
            .collect();
        Ok(rows)
    }

    // ==================================================================
    // Diagnostics: system statistics
    // ==================================================================

    /// Collect high-level system statistics for the admin overview.
    pub fn system_stats(&self) -> Result<SystemStats, StorageError> {
        let count = |sql: &str| -> Result<i64, StorageError> {
            Ok(self.conn.query_row(sql, [], |row| row.get(0))?)
        };

        let total_items = count("SELECT COUNT(*) FROM content_items")?;
        let total_feeds = count("SELECT COUNT(*) FROM feeds")?;
        let total_tags = count("SELECT COUNT(*) FROM tags")?;
        let total_collections = count("SELECT COUNT(*) FROM collections")?;
        let total_highlights =
            count("SELECT COUNT(*) FROM content_items WHERE content_type = 'highlight'")?;
        let total_notes = count("SELECT COUNT(*) FROM notes")?;
        let total_review_cards = count("SELECT COUNT(*) FROM review_cards")?;

        let page_count: i64 = self
            .conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let page_size: i64 = self
            .conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let db_size_bytes = page_count * page_size;

        let fts_ok = self
            .conn
            .execute(
                "INSERT INTO content_items_fts(content_items_fts) VALUES('integrity-check')",
                [],
            )
            .is_ok();

        let content_types = self.content_type_distribution()?;
        let statuses = self.status_distribution()?;

        Ok(SystemStats {
            total_items,
            total_feeds,
            total_tags,
            total_collections,
            total_highlights,
            total_notes,
            total_review_cards,
            db_size_bytes,
            fts_ok,
            content_types,
            statuses,
        })
    }

    /// Distribution of content items by content type, descending by count.
    pub fn content_type_distribution(&self) -> Result<Vec<ContentTypeCount>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT content_type, COUNT(*) FROM content_items
             GROUP BY content_type ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ContentTypeCount {
                content_type: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Distribution of content items by lifecycle status, descending by count.
    pub fn status_distribution(&self) -> Result<Vec<StatusCount>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT status, COUNT(*) FROM content_items
             GROUP BY status ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StatusCount {
                status: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ==================================================================
    // Diagnostics: broken links
    // ==================================================================

    /// List content items whose last link-health check recorded a problem
    /// (non-2xx HTTP status or a transport error), newest check first.
    pub fn list_broken_links(&self, limit: u32) -> Result<Vec<BrokenLinkRow>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT lh.content_item_id, ci.title, ci.url, lh.http_status,
                    lh.error_message, lh.last_checked_at
             FROM link_health lh
             JOIN content_items ci ON ci.id = lh.content_item_id
             WHERE lh.error_message IS NOT NULL
                OR lh.http_status IS NULL
                OR lh.http_status < 200
                OR lh.http_status >= 400
             ORDER BY lh.last_checked_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(BrokenLinkRow {
                content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                title: row.get(1)?,
                url: row.get(2)?,
                http_status: row.get(3)?,
                error_message: row.get(4)?,
                last_checked_at: parse_time(&row.get::<_, String>(5)?),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ==================================================================
    // Diagnostics: content-rules monitor
    // ==================================================================

    /// Build a rule-monitor report: every rule with the number of content
    /// items currently matching its filter.
    pub fn rule_monitor(&self) -> Result<Vec<RuleMonitorRow>, StorageError> {
        let rules = self.list_rules()?;
        let mut out = Vec::with_capacity(rules.len());
        for rule in rules {
            let match_count = self.count_items_matching_filter(&rule.filter_query)?;
            let action_summary = if rule.actions.is_empty() {
                "(none)".to_owned()
            } else {
                rule.actions
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push(RuleMonitorRow {
                rule_id: rule.id,
                name: rule.name,
                enabled: rule.enabled,
                priority: i64::from(rule.priority),
                filter_query: rule.filter_query,
                match_count,
                action_summary,
            });
        }
        Ok(out)
    }

    /// Count content items currently matching an arbitrary smart-filter query.
    pub fn count_items_matching_filter(&self, filter_query: &str) -> Result<i64, StorageError> {
        let smart = pergamon_core::smart_filter::SmartFilter::parse(filter_query)
            .map_err(|e| StorageError::Generic(format!("invalid filter: {e}")))?;
        let (sql, param_values) = build_smart_filter_count(&smart);
        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let count: i64 = stmt.query_row(param_refs.as_slice(), |row| row.get(0))?;
        Ok(count)
    }

    /// List all feed item metadata rows.
    #[allow(clippy::missing_errors_doc)]
    pub fn list_all_feed_item_meta(&self) -> Result<Vec<FeedItemMeta>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT content_item_id, feed_id, guid, summary FROM feed_item_meta")?;
        let rows = stmt.query_map([], |row| {
            Ok(FeedItemMeta {
                content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                feed_id: parse_uuid(&row.get::<_, String>(1)?),
                guid: row.get(2)?,
                summary: row.get(3)?,
            })
        })?;
        let mut metas = Vec::new();
        for row in rows {
            metas.push(row?);
        }
        Ok(metas)
    }

    /// List all bookmark metadata rows.
    #[allow(clippy::missing_errors_doc)]
    pub fn list_all_bookmark_meta(&self) -> Result<Vec<BookmarkMeta>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT content_item_id, original_url, saved_from, thumbnail_url, description, site_name, favicon_url
             FROM bookmark_meta",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BookmarkMeta {
                content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                original_url: row.get(1)?,
                saved_from: row.get(2)?,
                thumbnail_url: row.get(3)?,
                description: row.get(4)?,
                site_name: row.get(5)?,
                favicon_url: row.get(6)?,
            })
        })?;
        let mut metas = Vec::new();
        for row in rows {
            metas.push(row?);
        }
        Ok(metas)
    }

    /// List all highlight metadata rows.
    #[allow(clippy::missing_errors_doc)]
    pub fn list_all_highlight_meta(&self) -> Result<Vec<HighlightMeta>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT content_item_id, source_item_id, quote_text, note,
                    position_start, position_end, color
             FROM highlight_meta",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HighlightMeta {
                content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                source_item_id: row.get::<_, Option<String>>(1)?.map(|s| parse_uuid(&s)),
                quote_text: row.get(2)?,
                note: row.get(3)?,
                position_start: row.get(4)?,
                position_end: row.get(5)?,
                color: row.get(6)?,
            })
        })?;
        let mut metas = Vec::new();
        for row in rows {
            metas.push(row?);
        }
        Ok(metas)
    }

    /// List all content-item↔tag associations.
    #[allow(clippy::missing_errors_doc)]
    pub fn list_all_content_item_tags(&self) -> Result<Vec<(Uuid, Uuid)>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT content_item_id, tag_id FROM content_item_tags")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                parse_uuid(&row.get::<_, String>(0)?),
                parse_uuid(&row.get::<_, String>(1)?),
            ))
        })?;
        let mut pairs = Vec::new();
        for row in rows {
            pairs.push(row?);
        }
        Ok(pairs)
    }

    /// List all content-item↔collection associations (with sort order).
    #[allow(clippy::missing_errors_doc)]
    pub fn list_all_collection_items(&self) -> Result<Vec<(Uuid, Uuid, i32)>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT content_item_id, collection_id, sort_order FROM content_item_collections",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                parse_uuid(&row.get::<_, String>(0)?),
                parse_uuid(&row.get::<_, String>(1)?),
                row.get(2)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// Return the current migration version.
    #[allow(clippy::missing_errors_doc)]
    pub fn schema_version(&self) -> Result<i64, StorageError> {
        let version: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM __schema_migrations",
            [],
            |row| row.get(0),
        )?;
        Ok(version)
    }

    /// Check whether the database has any user data.
    #[allow(clippy::missing_errors_doc)]
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        let count: i64 = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM feeds) +
                    (SELECT COUNT(*) FROM content_items) +
                    (SELECT COUNT(*) FROM tags) +
                    (SELECT COUNT(*) FROM collections)",
            [],
            |row| row.get(0),
        )?;
        Ok(count == 0)
    }

    /// Restore a full backup into this database.
    ///
    /// The database must be empty. All inserts run inside a single
    /// transaction with foreign keys temporarily disabled.
    #[allow(clippy::missing_errors_doc, clippy::too_many_arguments)]
    pub fn restore_backup(
        &self,
        feed_folders: &[FeedFolder],
        feeds: &[Feed],
        content_items: &[ContentItem],
        tags: &[Tag],
        collections: &[Collection],
        feed_item_metas: &[FeedItemMeta],
        bookmark_metas: &[BookmarkMeta],
        highlight_metas: &[HighlightMeta],
        content_item_tags: &[(Uuid, Uuid)],
        collection_items: &[(Uuid, Uuid, i32)],
        notes: &[Note],
        review_cards: &[ReviewCard],
        review_logs: &[ReviewLog],
        rules: &[ContentRule],
    ) -> Result<(), StorageError> {
        if !self.is_empty()? {
            return Err(StorageError::Generic(
                "database is not empty — restore only works on a fresh database".into(),
            ));
        }

        // Disable FK checks for the import so we don't need topological ordering.
        self.conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let result = self.restore_backup_inner(
            feed_folders,
            feeds,
            content_items,
            tags,
            collections,
            feed_item_metas,
            bookmark_metas,
            highlight_metas,
            content_item_tags,
            collection_items,
            notes,
            review_cards,
            review_logs,
            rules,
        );
        // Re-enable FK checks regardless of success.
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        result
    }

    /// Inner restore implementation running inside a transaction.
    #[allow(clippy::too_many_arguments)]
    fn restore_backup_inner(
        &self,
        feed_folders: &[FeedFolder],
        feeds: &[Feed],
        content_items: &[ContentItem],
        tags: &[Tag],
        collections: &[Collection],
        feed_item_metas: &[FeedItemMeta],
        bookmark_metas: &[BookmarkMeta],
        highlight_metas: &[HighlightMeta],
        content_item_tags: &[(Uuid, Uuid)],
        collection_items: &[(Uuid, Uuid, i32)],
        notes: &[Note],
        review_cards: &[ReviewCard],
        review_logs: &[ReviewLog],
        rules: &[ContentRule],
    ) -> Result<(), StorageError> {
        self.conn.execute_batch("BEGIN;")?;

        let commit_or_rollback = |result: Result<(), StorageError>, conn: &Connection| {
            if result.is_ok() {
                conn.execute_batch("COMMIT;").map_err(StorageError::from)
            } else {
                let _ = conn.execute_batch("ROLLBACK;");
                result
            }
        };

        let result = (|| {
            for folder in feed_folders {
                self.insert_feed_folder(folder)?;
            }
            for feed in feeds {
                self.insert_feed(feed)?;
            }
            for item in content_items {
                self.insert_content_item(item)?;
            }
            for tag in tags {
                self.insert_tag(tag)?;
            }
            for coll in collections {
                self.insert_collection(coll)?;
            }
            for meta in feed_item_metas {
                self.insert_feed_item_meta(meta)?;
            }
            for meta in bookmark_metas {
                self.insert_bookmark_meta(meta)?;
            }
            for meta in highlight_metas {
                self.insert_highlight_meta(meta)?;
            }
            for note in notes {
                self.insert_note(note)?;
            }
            for card in review_cards {
                self.insert_review_card(card)?;
            }
            for log in review_logs {
                self.insert_review_log(log)?;
            }
            for rule in rules {
                self.insert_rule(rule)?;
            }
            for &(item_id, tag_id) in content_item_tags {
                self.conn.execute(
                    "INSERT OR IGNORE INTO content_item_tags (content_item_id, tag_id)
                     VALUES (?1, ?2)",
                    params![item_id.to_string(), tag_id.to_string()],
                )?;
            }
            for &(item_id, coll_id, sort) in collection_items {
                self.add_to_collection(item_id, coll_id, sort)?;
            }
            // Refresh FTS tags for items that have tag associations.
            // (insert_content_item already inserts the base FTS row.)
            for &(item_id, _) in content_item_tags {
                self.refresh_fts_tags(item_id)?;
            }
            Ok(())
        })();

        commit_or_rollback(result, &self.conn)
    }

    // ------------------------------------------------------------------
    // FTS5 search
    // ------------------------------------------------------------------

    /// Full-text search across content items.
    ///
    /// Returns results ranked by BM25 relevance.
    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>, StorageError> {
        // Quote each whitespace-delimited token so FTS5 treats hyphens and
        // other special characters as literals rather than operators.
        let safe_query: String = query
            .split_whitespace()
            .map(|token| {
                if token.contains('"') {
                    token.to_owned()
                } else {
                    format!("\"{token}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        let mut stmt = self.conn.prepare(
            "SELECT content_item_id, rank
             FROM content_items_fts
             WHERE content_items_fts MATCH ?1
             ORDER BY rank",
        )?;

        let rows = stmt.query_map(params![safe_query], |row| {
            Ok(SearchResult {
                content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                rank: row.get(1)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Full-text search with faceted filters and rich results.
    ///
    /// Returns matching content items ordered by BM25 relevance with
    /// recency as a tiebreaker. Each hit includes a snippet from the
    /// best-matching FTS column.
    #[allow(clippy::missing_errors_doc)]
    pub fn search_filtered(
        &self,
        query: &str,
        filter: &SearchFilter,
        limit: Option<u32>,
    ) -> Result<Vec<SearchHit>, StorageError> {
        let safe_query = quote_fts_tokens(query);
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }

        let (sql, param_values) = build_search_query(&safe_query, filter, limit);

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(SearchHit {
                item: row_to_content_item(row),
                rank: row.get(12)?,
                snippet: row.get(13)?,
            })
        })?;

        let mut hits = Vec::new();
        for row in rows {
            hits.push(row?);
        }
        Ok(hits)
    }

    // ------------------------------------------------------------------
    // Tags (listing)
    // ------------------------------------------------------------------

    /// List all tags, ordered by name.
    pub fn list_tags(&self) -> Result<Vec<Tag>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, created_at FROM tags ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |row| {
            Ok(Tag {
                id: parse_uuid(&row.get::<_, String>(0)?),
                name: row.get(1)?,
                created_at: parse_time(&row.get::<_, String>(2)?),
            })
        })?;
        let mut tags = Vec::new();
        for row in rows {
            tags.push(row?);
        }
        Ok(tags)
    }

    /// List tags matching a name prefix (for autocomplete).
    pub fn list_tags_matching(&self, prefix: &str) -> Result<Vec<Tag>, StorageError> {
        let pattern = format!("{prefix}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at FROM tags
             WHERE name LIKE ?1 COLLATE NOCASE
             ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok(Tag {
                id: parse_uuid(&row.get::<_, String>(0)?),
                name: row.get(1)?,
                created_at: parse_time(&row.get::<_, String>(2)?),
            })
        })?;
        let mut tags = Vec::new();
        for row in rows {
            tags.push(row?);
        }
        Ok(tags)
    }

    /// List all tags with their item counts, ordered by count descending.
    ///
    /// Returns a [`TagCount`] for every tag that has at least one associated
    /// content item. Tags with zero items are excluded.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn list_tags_with_counts(&self) -> Result<Vec<TagCount>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name, COUNT(*) AS cnt \
             FROM tags t \
             JOIN content_item_tags cit ON cit.tag_id = t.id \
             GROUP BY t.name \
             ORDER BY cnt DESC, t.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TagCount {
                tag_name: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ------------------------------------------------------------------
    // Bulk operations
    // ------------------------------------------------------------------

    /// Tag multiple content items with the same tag.
    ///
    /// Runs inside a transaction. Returns the number of new associations.
    pub fn bulk_tag(&self, item_ids: &[Uuid], tag_id: Uuid) -> Result<u64, StorageError> {
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| {
            let mut count: u64 = 0;
            for &item_id in item_ids {
                let affected = self.conn.execute(
                    "INSERT OR IGNORE INTO content_item_tags (content_item_id, tag_id)
                     VALUES (?1, ?2)",
                    params![item_id.to_string(), tag_id.to_string()],
                )?;
                if affected > 0 {
                    self.refresh_fts_tags(item_id)?;
                    count += 1;
                }
            }
            Ok(count)
        })();
        match result {
            Ok(count) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(count)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Add multiple content items to a collection.
    ///
    /// Runs inside a transaction. Returns the number of new memberships.
    pub fn bulk_add_to_collection(
        &self,
        item_ids: &[Uuid],
        collection_id: Uuid,
    ) -> Result<u64, StorageError> {
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| {
            let mut count: u64 = 0;
            for &item_id in item_ids {
                let affected = self.conn.execute(
                    "INSERT OR IGNORE INTO content_item_collections (content_item_id, collection_id, sort_order)
                     VALUES (?1, ?2, 0)",
                    params![item_id.to_string(), collection_id.to_string()],
                )?;
                #[allow(clippy::cast_sign_loss)]
                {
                    count += affected as u64;
                }
            }
            Ok(count)
        })();
        match result {
            Ok(count) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(count)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Archive multiple content items (set status to `archived`).
    ///
    /// Runs inside a transaction. Returns the number of items updated.
    pub fn bulk_archive(&self, item_ids: &[Uuid]) -> Result<u64, StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| {
            let mut count: u64 = 0;
            for &item_id in item_ids {
                let affected = self.conn.execute(
                    "UPDATE content_items SET status = 'archived', updated_at = ?1 WHERE id = ?2",
                    params![now, item_id.to_string()],
                )?;
                #[allow(clippy::cast_sign_loss)]
                {
                    count += affected as u64;
                }
            }
            Ok(count)
        })();
        match result {
            Ok(count) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(count)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Discard multiple content items (set status to `discarded`).
    ///
    /// This is a soft delete — items remain in the database but are
    /// excluded from active views. Runs inside a transaction.
    /// Returns the number of items updated.
    pub fn bulk_discard(&self, item_ids: &[Uuid]) -> Result<u64, StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| {
            let mut count: u64 = 0;
            for &item_id in item_ids {
                let affected = self.conn.execute(
                    "UPDATE content_items SET status = 'discarded', updated_at = ?1 WHERE id = ?2",
                    params![now, item_id.to_string()],
                )?;
                #[allow(clippy::cast_sign_loss)]
                {
                    count += affected as u64;
                }
            }
            Ok(count)
        })();
        match result {
            Ok(count) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(count)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------------
    // FTS5 helpers (private)
    // ------------------------------------------------------------------

    /// Insert or replace the FTS5 row for a content item.
    fn upsert_fts(&self, item: &ContentItem) -> Result<(), StorageError> {
        let tags = self.tags_for_item_as_string(item.id)?;
        self.conn.execute(
            "INSERT INTO content_items_fts (content_item_id, title, author, content_text, tags)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                item.id.to_string(),
                item.title,
                item.author,
                item.content_text,
                tags,
            ],
        )?;
        Ok(())
    }

    /// Refresh the tags column in the FTS index for an item.
    fn refresh_fts_tags(&self, content_item_id: Uuid) -> Result<(), StorageError> {
        let id_str = content_item_id.to_string();
        let tags = self.tags_for_item_as_string(content_item_id)?;

        // Fetch current FTS row values for the non-tag columns.
        let (title, author, content_text): (String, Option<String>, Option<String>) =
            self.conn.query_row(
                "SELECT title, author, content_text FROM content_items WHERE id = ?1",
                params![&id_str],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;

        // Delete old FTS row and insert updated one.
        self.conn.execute(
            "DELETE FROM content_items_fts WHERE content_item_id = ?1",
            params![&id_str],
        )?;
        self.conn.execute(
            "INSERT INTO content_items_fts (content_item_id, title, author, content_text, tags)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&id_str, title, author, content_text, tags],
        )?;

        Ok(())
    }

    /// Build a comma-separated tag string for a content item.
    fn tags_for_item_as_string(&self, content_item_id: Uuid) -> Result<String, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t
             JOIN content_item_tags ct ON ct.tag_id = t.id
             WHERE ct.content_item_id = ?1
             ORDER BY t.name",
        )?;
        let names: Vec<String> = stmt
            .query_map(params![content_item_id.to_string()], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names.join(", "))
    }

    // ------------------------------------------------------------------
    // Duplicate detection & merge (#16)
    // ------------------------------------------------------------------

    /// List all content item IDs and their URLs (for dedup scanning).
    ///
    /// Only items with a non-NULL URL are returned.
    pub fn list_all_urls(&self) -> Result<Vec<(Uuid, String)>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url FROM content_items WHERE url IS NOT NULL ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let url: String = row.get(1)?;
            Ok((parse_uuid(&id), url))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Delete a content item and its FTS entry.
    ///
    /// Extension tables (`feed_item_meta`, `bookmark_meta`, etc.) and
    /// junction tables (`content_item_tags`, `content_item_collections`)
    /// are cleaned up via `ON DELETE CASCADE`.
    pub fn delete_content_item(&self, id: Uuid) -> Result<bool, StorageError> {
        // Remove FTS entry first (no CASCADE on virtual tables).
        self.conn.execute(
            "DELETE FROM content_items_fts WHERE content_item_id = ?1",
            params![id.to_string()],
        )?;

        let affected = self.conn.execute(
            "DELETE FROM content_items WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(affected > 0)
    }

    /// Transfer all tags from one content item to another.
    ///
    /// Uses `INSERT OR IGNORE` so pre-existing associations are kept.
    pub fn transfer_tags(&self, from_id: Uuid, to_id: Uuid) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO content_item_tags (content_item_id, tag_id)
             SELECT ?1, tag_id FROM content_item_tags WHERE content_item_id = ?2",
            params![to_id.to_string(), from_id.to_string()],
        )?;
        self.refresh_fts_tags(to_id)?;
        Ok(())
    }

    /// Transfer all collection memberships from one content item to another.
    ///
    /// Uses `INSERT OR IGNORE` so pre-existing associations are kept.
    pub fn transfer_collections(&self, from_id: Uuid, to_id: Uuid) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO content_item_collections (content_item_id, collection_id, sort_order)
             SELECT ?1, collection_id, sort_order FROM content_item_collections WHERE content_item_id = ?2",
            params![to_id.to_string(), from_id.to_string()],
        )?;
        Ok(())
    }

    /// Update a content item's `created_at` to an earlier timestamp.
    ///
    /// Only applies the update if `earlier` is before the current value.
    pub fn backdate_created_at(
        &self,
        id: Uuid,
        earlier: OffsetDateTime,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE content_items SET created_at = ?1
             WHERE id = ?2 AND created_at > ?1",
            params![fmt_time(earlier), id.to_string()],
        )?;
        Ok(())
    }

    /// Check whether bookmark metadata exists for a content item.
    pub fn has_bookmark_meta(&self, content_item_id: Uuid) -> Result<bool, StorageError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM bookmark_meta WHERE content_item_id = ?1)",
            params![content_item_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    /// Check whether feed item metadata exists for a content item.
    pub fn has_feed_item_meta(&self, content_item_id: Uuid) -> Result<bool, StorageError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM feed_item_meta WHERE content_item_id = ?1)",
            params![content_item_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    /// Upsert bookmark metadata (insert if absent, update if present).
    pub fn upsert_bookmark_meta(&self, meta: &BookmarkMeta) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO bookmark_meta (content_item_id, original_url, saved_from, thumbnail_url, description, site_name, favicon_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(content_item_id) DO UPDATE SET
                original_url = COALESCE(excluded.original_url, bookmark_meta.original_url),
                saved_from = COALESCE(excluded.saved_from, bookmark_meta.saved_from),
                thumbnail_url = COALESCE(excluded.thumbnail_url, bookmark_meta.thumbnail_url),
                description = COALESCE(excluded.description, bookmark_meta.description),
                site_name = COALESCE(excluded.site_name, bookmark_meta.site_name),
                favicon_url = COALESCE(excluded.favicon_url, bookmark_meta.favicon_url)",
            params![
                meta.content_item_id.to_string(),
                meta.original_url,
                meta.saved_from,
                meta.thumbnail_url,
                meta.description,
                meta.site_name,
                meta.favicon_url,
            ],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Link health checking (#17)
    // ------------------------------------------------------------------

    /// Upsert a link health check result.
    ///
    /// Inserts a new record or updates an existing one for the given
    /// content item.
    pub fn upsert_link_health(&self, health: &LinkHealth) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO link_health (content_item_id, http_status, final_url, redirect_count, last_checked_at, error_message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(content_item_id) DO UPDATE SET
                http_status = excluded.http_status,
                final_url = excluded.final_url,
                redirect_count = excluded.redirect_count,
                last_checked_at = excluded.last_checked_at,
                error_message = excluded.error_message",
            params![
                health.content_item_id.to_string(),
                health.http_status,
                health.final_url,
                health.redirect_count,
                fmt_time(health.last_checked_at),
                health.error_message,
            ],
        )?;
        Ok(())
    }

    /// Fetch the link-health record for a content item, if one exists.
    pub fn get_link_health(
        &self,
        content_item_id: Uuid,
    ) -> Result<Option<LinkHealth>, StorageError> {
        self.conn
            .query_row(
                "SELECT content_item_id, http_status, final_url, redirect_count,
                        last_checked_at, error_message
                 FROM link_health WHERE content_item_id = ?1",
                params![content_item_id.to_string()],
                |row| {
                    Ok(LinkHealth {
                        content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                        http_status: row.get(1)?,
                        final_url: row.get(2)?,
                        redirect_count: row.get(3)?,
                        last_checked_at: parse_time(&row.get::<_, String>(4)?),
                        error_message: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// List content item URLs that need a health check.
    ///
    /// When `stale_days` is `None`, returns **all** items with a URL.
    /// When set, returns only items with no health record or whose
    /// `last_checked_at` is older than the given number of days.
    ///
    /// Returns `(content_item_id, url, title)` triples ordered by
    /// `created_at`.
    pub fn list_urls_for_health_check(
        &self,
        stale_days: Option<u32>,
    ) -> Result<Vec<(Uuid, String, String)>, StorageError> {
        let mut result = Vec::new();

        match stale_days {
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT c.id, c.url, c.title
                     FROM content_items c
                     WHERE c.url IS NOT NULL
                     ORDER BY c.created_at",
                )?;
                let rows = stmt.query_map([], |row| {
                    let id: String = row.get(0)?;
                    let url: String = row.get(1)?;
                    let title: String = row.get(2)?;
                    Ok((parse_uuid(&id), url, title))
                })?;
                for row in rows {
                    result.push(row?);
                }
            }
            Some(days) => {
                let cutoff = OffsetDateTime::now_utc() - time::Duration::days(i64::from(days));
                let cutoff_str = fmt_time(cutoff);
                let mut stmt = self.conn.prepare(
                    "SELECT c.id, c.url, c.title
                     FROM content_items c
                     LEFT JOIN link_health lh ON lh.content_item_id = c.id
                     WHERE c.url IS NOT NULL
                       AND (lh.content_item_id IS NULL
                            OR lh.last_checked_at < ?1)
                     ORDER BY c.created_at",
                )?;
                let rows = stmt.query_map(params![cutoff_str], |row| {
                    let id: String = row.get(0)?;
                    let url: String = row.get(1)?;
                    let title: String = row.get(2)?;
                    Ok((parse_uuid(&id), url, title))
                })?;
                for row in rows {
                    result.push(row?);
                }
            }
        }

        Ok(result)
    }

    /// List all link health records with dead or errored status.
    ///
    /// Returns records where the HTTP status is 4xx/5xx or where the
    /// request failed entirely (no status).
    pub fn list_unhealthy_links(&self) -> Result<Vec<(LinkHealth, String)>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT lh.content_item_id, lh.http_status, lh.final_url,
                    lh.redirect_count, lh.last_checked_at, lh.error_message,
                    c.title
             FROM link_health lh
             JOIN content_items c ON c.id = lh.content_item_id
             WHERE lh.http_status IS NULL
                OR lh.http_status >= 400
             ORDER BY lh.http_status, c.title",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let health = LinkHealth {
                content_item_id: parse_uuid(&id),
                http_status: row.get(1)?,
                final_url: row.get(2)?,
                redirect_count: row.get(3)?,
                last_checked_at: parse_time(&row.get::<_, String>(4)?),
                error_message: row.get(5)?,
            };
            let title: String = row.get(6)?;
            Ok((health, title))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

/// Build a SQL query for content items matching a [`ContentItemFilter`].
///
/// Returns `(sql, param_values)` where `param_values` are positional strings.
fn build_content_item_query(
    select_clause: &str,
    filter: &ContentItemFilter,
    limit: Option<u32>,
    offset: Option<u32>,
) -> (String, Vec<String>) {
    let mut param_values: Vec<String> = Vec::new();
    let sort_by_source = filter.sort == ContentItemSort::SourceAsc;
    let needs_fim = filter.feed_id.is_some() || filter.folder_id.is_some() || sort_by_source;
    let needs_feeds = filter.folder_id.is_some() || sort_by_source;

    let mut sql = if needs_fim {
        format!(
            "{select_clause}
             FROM content_items ci
             LEFT JOIN feed_item_meta fim ON fim.content_item_id = ci.id"
        )
    } else {
        format!("{select_clause} FROM content_items ci")
    };

    if needs_feeds {
        sql.push_str(" LEFT JOIN feeds f ON f.id = fim.feed_id");
    }

    sql.push_str(" WHERE 1=1");

    if let Some(ct) = filter.content_type {
        param_values.push(ct.as_str().to_owned());
        let _ = write!(sql, " AND ci.content_type = ?{}", param_values.len());
    }
    if let Some(st) = filter.status {
        param_values.push(st.as_str().to_owned());
        let _ = write!(sql, " AND ci.status = ?{}", param_values.len());
    }
    if let Some(fid) = filter.feed_id {
        param_values.push(fid.to_string());
        let _ = write!(sql, " AND fim.feed_id = ?{}", param_values.len());
    }
    if let Some(fld) = filter.folder_id {
        param_values.push(fld.to_string());
        let _ = write!(sql, " AND f.folder_id = ?{}", param_values.len());
    }
    if let Some(tid) = filter.tag_id {
        param_values.push(tid.to_string());
        let _ = write!(
            sql,
            " AND EXISTS (SELECT 1 FROM content_item_tags cit \
             WHERE cit.content_item_id = ci.id AND cit.tag_id = ?{})",
            param_values.len()
        );
    }
    if let Some(cid) = filter.collection_id {
        param_values.push(cid.to_string());
        let _ = write!(
            sql,
            " AND EXISTS (SELECT 1 FROM content_item_collections cic \
             WHERE cic.content_item_id = ci.id AND cic.collection_id = ?{})",
            param_values.len()
        );
    }
    if filter.uncollected {
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM content_item_collections cic \
             WHERE cic.content_item_id = ci.id)",
        );
    }

    match filter.sort {
        ContentItemSort::CreatedDesc => sql.push_str(" ORDER BY ci.created_at DESC"),
        ContentItemSort::TitleAsc => {
            sql.push_str(" ORDER BY ci.title COLLATE NOCASE ASC, ci.created_at DESC");
        }
        ContentItemSort::SourceAsc => {
            sql.push_str(" ORDER BY f.title COLLATE NOCASE ASC, ci.created_at DESC");
        }
    }

    if let Some(lim) = limit {
        let _ = write!(sql, " LIMIT {lim}");
    }
    if let Some(off) = offset {
        let _ = write!(sql, " OFFSET {off}");
    }

    (sql, param_values)
}

/// Build a SQL query for a smart collection filter.
///
/// Translates [`SmartFilter`] predicates into a WHERE clause against
/// `content_items`. Returns `(sql, param_values)`.
fn build_smart_filter_query(
    filter: &pergamon_core::smart_filter::SmartFilter,
    limit: Option<u32>,
) -> (String, Vec<String>) {
    use pergamon_core::smart_filter::FilterPredicate;

    let mut param_values: Vec<String> = Vec::new();
    let mut sql = String::from(
        "SELECT ci.id, ci.url, ci.title, ci.author, ci.content_type, ci.status,
                ci.content_text, ci.excerpt, ci.published_at, ci.created_at, ci.updated_at, ci.read_at
         FROM content_items ci",
    );

    let has_source = filter
        .predicates()
        .iter()
        .any(|p| matches!(p, FilterPredicate::Source(_)));
    if has_source {
        sql.push_str(
            " LEFT JOIN feed_item_meta fim ON fim.content_item_id = ci.id
             LEFT JOIN feeds f ON f.id = fim.feed_id",
        );
    }

    let has_text = filter.has_text_query();
    if has_text {
        sql.push_str(" JOIN content_items_fts fts ON fts.rowid = ci.rowid");
    }

    sql.push_str(" WHERE 1=1");

    for pred in filter.predicates() {
        append_predicate_sql(&mut sql, &mut param_values, pred);
    }

    sql.push_str(" ORDER BY ci.created_at DESC");

    if let Some(lim) = limit {
        let _ = write!(sql, " LIMIT {lim}");
    }

    (sql, param_values)
}

/// Build a COUNT query for a smart collection filter.
fn build_smart_filter_count(
    filter: &pergamon_core::smart_filter::SmartFilter,
) -> (String, Vec<String>) {
    use pergamon_core::smart_filter::FilterPredicate;

    let mut param_values: Vec<String> = Vec::new();
    let mut sql = String::from("SELECT COUNT(*) FROM content_items ci");

    let has_source = filter
        .predicates()
        .iter()
        .any(|p| matches!(p, FilterPredicate::Source(_)));
    if has_source {
        sql.push_str(
            " LEFT JOIN feed_item_meta fim ON fim.content_item_id = ci.id
             LEFT JOIN feeds f ON f.id = fim.feed_id",
        );
    }

    let has_text = filter.has_text_query();
    if has_text {
        sql.push_str(" JOIN content_items_fts fts ON fts.rowid = ci.rowid");
    }

    sql.push_str(" WHERE 1=1");

    for pred in filter.predicates() {
        append_predicate_sql(&mut sql, &mut param_values, pred);
    }

    (sql, param_values)
}

/// Append SQL for a single filter predicate.
fn append_predicate_sql(
    sql: &mut String,
    params: &mut Vec<String>,
    pred: &pergamon_core::smart_filter::FilterPredicate,
) {
    use pergamon_core::smart_filter::FilterPredicate;

    match pred {
        FilterPredicate::ContentType(types) => {
            let placeholders: Vec<String> = types
                .iter()
                .map(|t| {
                    params.push(t.as_str().to_owned());
                    format!("?{}", params.len())
                })
                .collect();
            let _ = write!(sql, " AND ci.content_type IN ({})", placeholders.join(", "));
        }
        FilterPredicate::Tag(tags) => {
            let placeholders: Vec<String> = tags
                .iter()
                .map(|t| {
                    params.push(t.clone());
                    format!("?{}", params.len())
                })
                .collect();
            let _ = write!(
                sql,
                " AND EXISTS (SELECT 1 FROM content_item_tags cit \
                 JOIN tags t ON t.id = cit.tag_id \
                 WHERE cit.content_item_id = ci.id \
                 AND t.name IN ({}))",
                placeholders.join(", ")
            );
        }
        FilterPredicate::Status(statuses) => {
            let placeholders: Vec<String> = statuses
                .iter()
                .map(|s| {
                    params.push(s.as_str().to_owned());
                    format!("?{}", params.len())
                })
                .collect();
            let _ = write!(sql, " AND ci.status IN ({})", placeholders.join(", "));
        }
        FilterPredicate::ExcludeStatus(statuses) => {
            let placeholders: Vec<String> = statuses
                .iter()
                .map(|s| {
                    params.push(s.as_str().to_owned());
                    format!("?{}", params.len())
                })
                .collect();
            let _ = write!(sql, " AND ci.status NOT IN ({})", placeholders.join(", "));
        }
        FilterPredicate::Source(src) => {
            params.push(format!("%{src}%"));
            let _ = write!(sql, " AND f.title LIKE ?{}", params.len());
        }
        FilterPredicate::CreatedSince(date) => {
            params.push(format!("{date}T00:00:00Z"));
            let _ = write!(sql, " AND ci.created_at >= ?{}", params.len());
        }
        FilterPredicate::CreatedBefore(date) => {
            params.push(format!("{date}T00:00:00Z"));
            let _ = write!(sql, " AND ci.created_at < ?{}", params.len());
        }
        FilterPredicate::Text(query) => {
            let safe = quote_fts_tokens(query);
            params.push(safe);
            let _ = write!(sql, " AND fts MATCH ?{}", params.len());
        }
    }
}
///
/// When a subquery with `?1, ?2, ...` is embedded inside a larger statement
/// that already uses `?1` and `?2` (e.g. for `new_status` and `now`), this
/// shifts the subquery placeholders so they don't collide.
fn reindex_params(sql: &str, offset: usize) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '?' {
            // Collect digits after '?'
            let mut digits = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    digits.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            if let Ok(n) = digits.parse::<usize>() {
                let _ = write!(result, "?{}", n + offset);
            } else {
                result.push('?');
                result.push_str(&digits);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Quote FTS5 query tokens so hyphens and special chars are treated as literals.
///
/// Each whitespace-delimited token is wrapped in double quotes unless it
/// already contains a quote character.
fn quote_fts_tokens(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| {
            if token.contains('"') {
                token.to_owned()
            } else {
                format!("\"{token}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a SQL query for full-text search with faceted filters.
///
/// Returns `(sql, param_values)`. The result columns are the standard
/// 11 content-item columns (indices 0–10) followed by `rank` (11) and
/// `snippet` (12).
fn build_search_query(
    safe_query: &str,
    filter: &SearchFilter,
    limit: Option<u32>,
) -> (String, Vec<String>) {
    let mut param_values: Vec<String> = Vec::new();

    // ?1 is always the FTS MATCH query.
    param_values.push(safe_query.to_owned());

    let mut sql = String::from(
        "SELECT ci.id, ci.url, ci.title, ci.author, ci.content_type, ci.status,
                ci.content_text, ci.excerpt, ci.published_at, ci.created_at, ci.updated_at, ci.read_at,
                fts.rank,
                snippet(content_items_fts, -1, '»', '«', '…', 20) AS snip
         FROM content_items_fts fts
         JOIN content_items ci ON ci.id = fts.content_item_id",
    );

    // Conditional JOINs.
    if filter.tag_name.is_some() {
        sql.push_str(
            " JOIN content_item_tags cit ON cit.content_item_id = ci.id
              JOIN tags t ON t.id = cit.tag_id",
        );
    }
    if filter.feed_id.is_some() {
        sql.push_str(" JOIN feed_item_meta fim ON fim.content_item_id = ci.id");
    }

    sql.push_str(" WHERE content_items_fts MATCH ?1");

    if let Some(ct) = filter.content_type {
        param_values.push(ct.as_str().to_owned());
        let _ = write!(sql, " AND ci.content_type = ?{}", param_values.len());
    }
    if let Some(st) = filter.status {
        param_values.push(st.as_str().to_owned());
        let _ = write!(sql, " AND ci.status = ?{}", param_values.len());
    }
    if let Some(ref tag) = filter.tag_name {
        param_values.push(tag.clone());
        let _ = write!(sql, " AND t.name = ?{} COLLATE NOCASE", param_values.len());
    }
    if let Some(fid) = filter.feed_id {
        param_values.push(fid.to_string());
        let _ = write!(sql, " AND fim.feed_id = ?{}", param_values.len());
    }
    if let Some(since) = filter.since {
        param_values.push(fmt_time(since));
        let _ = write!(sql, " AND ci.created_at >= ?{}", param_values.len());
    }
    if let Some(before) = filter.before {
        param_values.push(fmt_time(before));
        let _ = write!(sql, " AND ci.created_at < ?{}", param_values.len());
    }

    // Relevance first, recency as tiebreaker.
    sql.push_str(" ORDER BY fts.rank, COALESCE(ci.published_at, ci.created_at) DESC");

    if let Some(lim) = limit {
        let _ = write!(sql, " LIMIT {lim}");
    }

    (sql, param_values)
}

// ======================================================================
// Helper functions
// ======================================================================

/// Format an `OffsetDateTime` as an RFC 3339 string for `SQLite` storage.
fn fmt_time(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

/// Parse a UUID string from the database.
///
/// # Panics
///
/// This function will never panic in practice because only valid UUIDs
/// are written by the storage layer. The `unwrap_or_else` + `unreachable!`
/// satisfies the no-panic lint while documenting the invariant.
fn parse_uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap_or_else(|_| unreachable!("invalid UUID in database: {s:?}"))
}

/// Parse an RFC 3339 timestamp string from the database.
///
/// # Panics
///
/// This function will never panic in practice because only valid timestamps
/// are written by the storage layer.
fn parse_time(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339)
        .unwrap_or_else(|_| unreachable!("invalid timestamp in database: {s:?}"))
}

/// Convert a calendar date to a Julian Day Number for simple date arithmetic.
#[allow(clippy::many_single_char_names, clippy::cast_possible_wrap)]
const fn julian_day(year: i32, month: u32, day: u32) -> i32 {
    let a = 14_i32.wrapping_sub(month as i32) / 12;
    let y = year + 4800 - a;
    let m = month as i32 + 12 * a - 3;
    day as i32 + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32_045
}

/// Convert a Julian Day Number back to (year, month, day).
#[allow(clippy::many_single_char_names, clippy::cast_sign_loss)]
const fn from_julian_day(jd: i32) -> (i32, u32, u32) {
    let a = jd + 32_044;
    let b = (4 * a + 3) / 146_097;
    let c = a - (146_097 * b) / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5 + 1) as u32;
    let month = (m + 3 - 12 * (m / 10)) as u32;
    let year = 100 * b + d - 4800 + m / 10;
    (year, month, day)
}

/// Convert a word count to estimated reading minutes at 238 WPM.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn words_to_minutes(words: i64) -> i64 {
    if words <= 0 {
        return 0;
    }
    (words as f64 / 238.0).ceil() as i64
}

/// Compute the longest consecutive-day streak from a sorted (descending) list of date strings.
fn compute_longest_streak(dates: &[String]) -> i64 {
    if dates.is_empty() {
        return 0;
    }
    // Parse all dates into Julian Day Numbers for arithmetic.
    let jds: Vec<i32> = dates
        .iter()
        .filter_map(|d| {
            let y: i32 = d.get(..4)?.parse().ok()?;
            let m: u32 = d.get(5..7)?.parse().ok()?;
            let day: u32 = d.get(8..10)?.parse().ok()?;
            Some(julian_day(y, m, day))
        })
        .collect();

    if jds.is_empty() {
        return 0;
    }

    // Sort ascending for forward walk.
    let mut sorted = jds;
    sorted.sort_unstable();
    sorted.dedup();

    let mut longest: i64 = 1;
    let mut current: i64 = 1;
    for window in sorted.windows(2) {
        if window[1] - window[0] == 1 {
            current += 1;
            if current > longest {
                longest = current;
            }
        } else {
            current = 1;
        }
    }
    longest
}

/// Compute current and longest reading streaks from a sorted list of date strings.
///
/// Returns `(current_streak, longest_streak)`. The current streak counts
/// backwards from `today`; it is 0 if the user didn't read today or yesterday.
#[must_use]
pub fn compute_reading_streaks(dates: &[String], today: &str) -> (i64, i64) {
    if dates.is_empty() {
        return (0, 0);
    }

    let parse_jd = |d: &str| -> Option<i32> {
        let y: i32 = d.get(..4)?.parse().ok()?;
        let m: u32 = d.get(5..7)?.parse().ok()?;
        let day: u32 = d.get(8..10)?.parse().ok()?;
        Some(julian_day(y, m, day))
    };

    let mut jds: Vec<i32> = dates.iter().filter_map(|d| parse_jd(d)).collect();
    if jds.is_empty() {
        return (0, 0);
    }
    jds.sort_unstable();
    jds.dedup();

    // Longest streak.
    let mut longest: i64 = 1;
    let mut current: i64 = 1;
    for window in jds.windows(2) {
        if window[1] - window[0] == 1 {
            current += 1;
            if current > longest {
                longest = current;
            }
        } else {
            current = 1;
        }
    }

    // Current streak: walk backwards from today.
    let today_jd = parse_jd(today).unwrap_or(0);
    let last = *jds.last().unwrap_or(&0);
    // The streak is alive if the last read date is today or yesterday.
    if today_jd - last > 1 {
        return (0, longest);
    }
    let mut cur: i64 = 1;
    let mut prev = last;
    for &jd in jds.iter().rev().skip(1) {
        if prev - jd == 1 {
            cur += 1;
            prev = jd;
        } else {
            break;
        }
    }
    (cur, longest)
}

/// Map a rusqlite `Row` to a `ContentItem`.
fn row_to_content_item(row: &rusqlite::Row<'_>) -> ContentItem {
    ContentItem {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        url: row.get(1).unwrap_or_default(),
        title: row.get(2).unwrap_or_default(),
        author: row.get(3).unwrap_or_default(),
        content_type: row
            .get::<_, String>(4)
            .unwrap_or_default()
            .parse()
            .unwrap_or(ContentType::Article),
        status: row
            .get::<_, String>(5)
            .unwrap_or_default()
            .parse()
            .unwrap_or(DocumentStatus::Inbox),
        content_text: row.get(6).unwrap_or_default(),
        excerpt: row.get(7).unwrap_or_default(),
        published_at: row
            .get::<_, Option<String>>(8)
            .unwrap_or_default()
            .map(|s| parse_time(&s)),
        created_at: parse_time(&row.get::<_, String>(9).unwrap_or_default()),
        updated_at: parse_time(&row.get::<_, String>(10).unwrap_or_default()),
        read_at: row
            .get::<_, Option<String>>(11)
            .unwrap_or_default()
            .map(|s| parse_time(&s)),
    }
}

/// Map a rusqlite `Row` to a `Feed`.
///
/// Expected column order: `id`, `title`, `url`, `site_url`, `description`, `etag`,
/// `last_modified_header`, `error_count`, `last_error`, `last_fetched_at`,
/// `folder_id`, `created_at`, `updated_at`.
fn row_to_feed(row: &rusqlite::Row<'_>) -> Feed {
    Feed {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        title: row.get(1).unwrap_or_default(),
        url: row.get(2).unwrap_or_default(),
        site_url: row.get(3).unwrap_or_default(),
        description: row.get(4).unwrap_or_default(),
        etag: row.get(5).unwrap_or_default(),
        last_modified_header: row.get(6).unwrap_or_default(),
        error_count: row.get(7).unwrap_or_default(),
        last_error: row.get(8).unwrap_or_default(),
        last_fetched_at: row
            .get::<_, Option<String>>(9)
            .unwrap_or_default()
            .map(|s| parse_time(&s)),
        folder_id: row
            .get::<_, Option<String>>(10)
            .unwrap_or_default()
            .map(|s| parse_uuid(&s)),
        created_at: parse_time(&row.get::<_, String>(11).unwrap_or_default()),
        updated_at: parse_time(&row.get::<_, String>(12).unwrap_or_default()),
    }
}

/// Map a rusqlite `Row` to a [`FeedFolder`].
///
/// Expected column order: `id`, `name`, `parent_id`, `created_at`, `updated_at`.
fn row_to_feed_folder(row: &rusqlite::Row<'_>) -> FeedFolder {
    FeedFolder {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        name: row.get(1).unwrap_or_default(),
        parent_id: row
            .get::<_, Option<String>>(2)
            .unwrap_or_default()
            .map(|s| parse_uuid(&s)),
        created_at: parse_time(&row.get::<_, String>(3).unwrap_or_default()),
        updated_at: parse_time(&row.get::<_, String>(4).unwrap_or_default()),
    }
}

/// Map a rusqlite `Row` to a [`Note`].
///
/// Expected column order: `id`, `content_item_id`, `body`, `created_at`, `updated_at`.
fn row_to_note(row: &rusqlite::Row<'_>) -> Note {
    Note {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        content_item_id: parse_uuid(&row.get::<_, String>(1).unwrap_or_default()),
        body: row.get(2).unwrap_or_default(),
        created_at: parse_time(&row.get::<_, String>(3).unwrap_or_default()),
        updated_at: parse_time(&row.get::<_, String>(4).unwrap_or_default()),
    }
}

/// Truncate a string for use as a content item title.
///
/// Takes the first 80 characters (or up to the first newline) and appends
/// an ellipsis if truncated.
fn truncate_for_title(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or(text);
    if first_line.len() <= 80 {
        first_line.to_owned()
    } else {
        let truncated: String = first_line.chars().take(77).collect();
        format!("{truncated}…")
    }
}

/// Map a rusqlite `Row` to a [`ReviewCard`].
///
/// Expected column order: `id`, `content_item_id`, `state`, `stability`,
/// `difficulty`, `due_at`, `last_reviewed_at`, `review_count`, `lapse_count`,
/// `scheduled_days`, `created_at`, `updated_at`.
fn row_to_review_card(row: &rusqlite::Row<'_>) -> ReviewCard {
    ReviewCard {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        content_item_id: parse_uuid(&row.get::<_, String>(1).unwrap_or_default()),
        state: row
            .get::<_, String>(2)
            .unwrap_or_default()
            .parse::<CardState>()
            .unwrap_or(CardState::New),
        stability: row.get(3).unwrap_or_default(),
        difficulty: row.get(4).unwrap_or_default(),
        due_at: parse_time(&row.get::<_, String>(5).unwrap_or_default()),
        last_reviewed_at: row
            .get::<_, Option<String>>(6)
            .unwrap_or_default()
            .map(|s| parse_time(&s)),
        review_count: row.get(7).unwrap_or_default(),
        lapse_count: row.get(8).unwrap_or_default(),
        scheduled_days: row.get(9).unwrap_or_default(),
        created_at: parse_time(&row.get::<_, String>(10).unwrap_or_default()),
        updated_at: parse_time(&row.get::<_, String>(11).unwrap_or_default()),
    }
}

/// Map a rusqlite `Row` to a [`ReviewLog`].
///
/// Expected column order: `id`, `card_id`, `rating`, `state_before`,
/// `stability_before`, `difficulty_before`, `state_after`, `stability_after`,
/// `difficulty_after`, `elapsed_days`, `scheduled_days`, `reviewed_at`.
fn row_to_review_log(row: &rusqlite::Row<'_>) -> ReviewLog {
    ReviewLog {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        card_id: parse_uuid(&row.get::<_, String>(1).unwrap_or_default()),
        rating: Rating::from_value(row.get::<_, u32>(2).unwrap_or(3)).unwrap_or(Rating::Good),
        state_before: row
            .get::<_, String>(3)
            .unwrap_or_default()
            .parse::<CardState>()
            .unwrap_or(CardState::New),
        stability_before: row.get(4).unwrap_or_default(),
        difficulty_before: row.get(5).unwrap_or_default(),
        state_after: row
            .get::<_, String>(6)
            .unwrap_or_default()
            .parse::<CardState>()
            .unwrap_or(CardState::New),
        stability_after: row.get(7).unwrap_or_default(),
        difficulty_after: row.get(8).unwrap_or_default(),
        elapsed_days: row.get(9).unwrap_or_default(),
        scheduled_days: row.get(10).unwrap_or_default(),
        reviewed_at: parse_time(&row.get::<_, String>(11).unwrap_or_default()),
    }
}

fn row_to_rule(row: &rusqlite::Row<'_>) -> ContentRule {
    let actions_json: String = row.get(5).unwrap_or_default();
    let actions: Vec<RuleAction> = serde_json::from_str(&actions_json).unwrap_or_default();
    ContentRule {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        name: row.get(1).unwrap_or_default(),
        enabled: row.get::<_, i32>(2).unwrap_or(1) != 0,
        priority: row.get(3).unwrap_or_default(),
        filter_query: row.get(4).unwrap_or_default(),
        actions,
        created_at: parse_time(&row.get::<_, String>(6).unwrap_or_default()),
        updated_at: parse_time(&row.get::<_, String>(7).unwrap_or_default()),
    }
}

/// Map a row from `import_log` to an [`ImportLogEntry`].
fn row_to_import_log(row: &rusqlite::Row<'_>) -> ImportLogEntry {
    let source = ImportSource::from_db_str(&row.get::<_, String>(1).unwrap_or_default())
        .unwrap_or(ImportSource::Backup);
    ImportLogEntry {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        source,
        file_name: row.get(2).unwrap_or_default(),
        items_added: row.get(3).unwrap_or_default(),
        items_existing: row.get(4).unwrap_or_default(),
        items_skipped: row.get(5).unwrap_or_default(),
        errors: row.get(6).unwrap_or_default(),
        error_detail: row.get(7).unwrap_or_default(),
        dry_run: row.get::<_, i32>(8).unwrap_or(0) != 0,
        created_at: parse_time(&row.get::<_, String>(9).unwrap_or_default()),
    }
}

/// Map a row from `extraction_log` to an [`ExtractionEvent`].
fn row_to_extraction_event(row: &rusqlite::Row<'_>) -> ExtractionEvent {
    let source = ExtractionSource::from_db_str(&row.get::<_, String>(3).unwrap_or_default())
        .unwrap_or(ExtractionSource::Save);
    let content_item_id = row
        .get::<_, Option<String>>(1)
        .unwrap_or_default()
        .map(|s| parse_uuid(&s));
    ExtractionEvent {
        id: parse_uuid(&row.get::<_, String>(0).unwrap_or_default()),
        content_item_id,
        url: row.get(2).unwrap_or_default(),
        source,
        success: row.get::<_, i32>(4).unwrap_or(0) != 0,
        extractor: row.get(5).unwrap_or_default(),
        error_message: row.get(6).unwrap_or_default(),
        created_at: parse_time(&row.get::<_, String>(7).unwrap_or_default()),
    }
}

#[cfg(test)]
mod wal_tests {
    use super::*;

    /// Opening a file-backed database enables WAL journal mode and sets a
    /// non-zero busy timeout for concurrent access (issue #83).
    #[test]
    fn open_enables_wal_and_busy_timeout() {
        let mut path = std::env::temp_dir();
        path.push(format!("pergamon-wal-test-{}.db", Uuid::new_v4()));

        let db =
            Database::open(&path).unwrap_or_else(|e| unreachable!("failed to open file DB: {e}"));

        let journal_mode: String = db
            .conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
            .unwrap_or_else(|e| unreachable!("query journal_mode: {e}"));
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let busy_timeout: i64 = db
            .conn
            .query_row("PRAGMA busy_timeout;", [], |row| row.get(0))
            .unwrap_or_else(|e| unreachable!("query busy_timeout: {e}"));
        assert_eq!(busy_timeout, 5000);

        // Clean up the database and its WAL/SHM sidecar files.
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }
}
