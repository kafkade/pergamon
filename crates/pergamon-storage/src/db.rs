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
use pergamon_core::model::{
    BookmarkMeta, Collection, ContentItem, Feed, FeedItemMeta, HighlightMeta, SearchResult, Tag,
};
use pergamon_core::status::DocumentStatus;

use crate::error::StorageError;

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

    // ------------------------------------------------------------------
    // Feeds
    // ------------------------------------------------------------------

    /// Insert a new feed.
    pub fn insert_feed(&self, feed: &Feed) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO feeds (id, title, url, site_url, description, etag,
                last_modified_header, error_count, last_error,
                last_fetched_at, last_successful_fetch_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                        last_fetched_at, created_at, updated_at
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
                        last_fetched_at, created_at, updated_at
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
                    last_fetched_at, created_at, updated_at
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

    // ------------------------------------------------------------------
    // Content items
    // ------------------------------------------------------------------

    /// Insert a new content item and update the FTS5 index.
    pub fn insert_content_item(&self, item: &ContentItem) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO content_items
                (id, url, title, author, content_type, status, content_text, excerpt, published_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                        content_text, excerpt, published_at, created_at, updated_at
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
                    content_text, excerpt, published_at, created_at, updated_at
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
    pub fn update_content_item_status(
        &self,
        id: Uuid,
        status: DocumentStatus,
    ) -> Result<(), StorageError> {
        let now = fmt_time(OffsetDateTime::now_utc());
        let affected = self.conn.execute(
            "UPDATE content_items SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now, id.to_string()],
        )?;
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
            "INSERT INTO bookmark_meta (content_item_id, original_url, saved_from, thumbnail_url, description)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                meta.content_item_id.to_string(),
                meta.original_url,
                meta.saved_from,
                meta.thumbnail_url,
                meta.description,
            ],
        )?;
        Ok(())
    }

    /// Retrieve bookmark metadata by content item ID.
    pub fn get_bookmark_meta(&self, content_item_id: Uuid) -> Result<BookmarkMeta, StorageError> {
        self.conn
            .query_row(
                "SELECT content_item_id, original_url, saved_from, thumbnail_url, description
                 FROM bookmark_meta WHERE content_item_id = ?1",
                params![content_item_id.to_string()],
                |row| {
                    Ok(BookmarkMeta {
                        content_item_id: parse_uuid(&row.get::<_, String>(0)?),
                        original_url: row.get(1)?,
                        saved_from: row.get(2)?,
                        thumbnail_url: row.get(3)?,
                        description: row.get(4)?,
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

    // ------------------------------------------------------------------
    // Collections
    // ------------------------------------------------------------------

    /// Insert a new collection.
    pub fn insert_collection(&self, coll: &Collection) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO collections (id, name, parent_id, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                coll.id.to_string(),
                coll.name,
                coll.parent_id.map(|id| id.to_string()),
                coll.sort_order,
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
                "SELECT id, name, parent_id, sort_order, created_at, updated_at
                 FROM collections WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok(Collection {
                        id: parse_uuid(&row.get::<_, String>(0)?),
                        name: row.get(1)?,
                        parent_id: row.get::<_, Option<String>>(2)?.map(|s| parse_uuid(&s)),
                        sort_order: row.get(3)?,
                        created_at: parse_time(&row.get::<_, String>(4)?),
                        updated_at: parse_time(&row.get::<_, String>(5)?),
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
    pub fn add_to_collection(
        &self,
        content_item_id: Uuid,
        collection_id: Uuid,
        sort_order: i32,
    ) -> Result<(), StorageError> {
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
    }
}

/// Map a rusqlite `Row` to a `Feed`.
///
/// Expected column order: `id`, `title`, `url`, `site_url`, `description`, `etag`,
/// `last_modified_header`, `error_count`, `last_error`, `last_fetched_at`,
/// `created_at`, `updated_at`.
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
        created_at: parse_time(&row.get::<_, String>(10).unwrap_or_default()),
        updated_at: parse_time(&row.get::<_, String>(11).unwrap_or_default()),
    }
}
