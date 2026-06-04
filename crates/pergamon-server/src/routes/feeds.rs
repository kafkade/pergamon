// SPDX-License-Identifier: AGPL-3.0-only

//! Feed management API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::model::{ContentItem, Feed, FeedFolder, FeedItemMeta};
use pergamon_core::status::DocumentStatus;

use crate::error::ApiError;
use crate::state::AppState;

// ======================================================================
// Request / response types
// ======================================================================

/// Request body for subscribing to a feed.
#[derive(Debug, Deserialize)]
pub struct SubscribeFeedRequest {
    /// Feed URL (RSS/Atom endpoint).
    pub url: String,
}

/// Request body for triggering feed sync.
#[derive(Debug, Deserialize)]
pub struct SyncFeedsRequest {
    /// Optional feed ID to sync a specific feed. Syncs all if omitted.
    pub feed_id: Option<Uuid>,
}

/// Response for feed sync operation.
#[derive(Debug, Serialize)]
pub struct SyncResponse {
    /// Number of feeds synced.
    pub feeds_synced: u64,
    /// Number of new items ingested.
    pub new_items: u64,
    /// Number of feeds that errored.
    pub errors: u64,
}

/// Response for OPML import.
#[derive(Debug, Serialize)]
pub struct OpmlImportResponse {
    /// Number of folders created.
    pub folders_created: u64,
    /// Number of folders that already existed.
    pub folders_existing: u64,
    /// Number of feeds added.
    pub feeds_added: u64,
    /// Number of feeds that already existed.
    pub feeds_existing: u64,
    /// Number of feeds moved to a different folder.
    pub feeds_moved: u64,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/feeds` — list all feed subscriptions.
pub async fn list_feeds(State(state): State<AppState>) -> Result<Json<Vec<Feed>>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let feeds = db.list_feeds()?;
    Ok(Json(feeds))
}

/// `POST /api/feeds` — subscribe to a feed URL.
pub async fn subscribe_feed(
    State(state): State<AppState>,
    Json(body): Json<SubscribeFeedRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate URL.
    let parsed_url: url::Url = body
        .url
        .parse()
        .map_err(|_| ApiError::bad_request("invalid URL"))?;
    let scheme = parsed_url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(ApiError::bad_request(
            "only http and https URLs are supported",
        ));
    }

    // Check for duplicate.
    {
        let db = state
            .db
            .lock()
            .map_err(|_| ApiError::internal("database lock poisoned"))?;
        if let Some(existing) = db.get_feed_by_url(&body.url)? {
            return Ok((StatusCode::OK, Json(existing)).into_response());
        }
    }

    // Fetch and parse the feed.
    let response = state
        .http
        .get(body.url.as_str())
        .send()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("failed to fetch feed: {e}")))?;

    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let final_url = response.url().to_string();

    if !response.status().is_success() {
        return Err(ApiError::bad_gateway(format!(
            "upstream returned HTTP {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("failed to read feed response: {e}")))?;

    let parsed = pergamon_feed::parse_feed(&bytes, &final_url)
        .map_err(|e| ApiError::unprocessable(format!("failed to parse feed: {e}")))?;

    let now = OffsetDateTime::now_utc();
    let feed = Feed {
        id: Uuid::new_v4(),
        title: parsed.title.clone(),
        url: final_url,
        site_url: parsed.site_url.clone(),
        description: parsed.description.clone(),
        etag,
        last_modified_header: last_modified,
        error_count: 0,
        last_error: None,
        last_fetched_at: Some(now),
        folder_id: None,
        created_at: now,
        updated_at: now,
    };

    // Insert feed and ingest entries.
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    db.insert_feed(&feed)?;
    ingest_entries(&db, &feed, &parsed.entries)?;
    db.update_feed_fetch_success(
        feed.id,
        feed.etag.as_deref(),
        feed.last_modified_header.as_deref(),
    )?;

    Ok((StatusCode::CREATED, Json(feed)).into_response())
}

/// `DELETE /api/feeds/{id}` — unsubscribe from a feed.
pub async fn delete_feed(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let deleted = db.delete_feed(id)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("feed not found"))
    }
}

/// `POST /api/feeds/sync` — trigger feed sync.
pub async fn sync_feeds(
    State(state): State<AppState>,
    Json(body): Json<SyncFeedsRequest>,
) -> Result<Json<SyncResponse>, ApiError> {
    // Collect feeds to sync.
    let feeds = {
        let db = state
            .db
            .lock()
            .map_err(|_| ApiError::internal("database lock poisoned"))?;
        if let Some(feed_id) = body.feed_id {
            let feed = db
                .get_feed(feed_id)
                .map_err(|_| ApiError::not_found("feed not found"))?;
            vec![feed]
        } else {
            db.list_feeds()?
        }
    };

    let mut total_new: u64 = 0;
    let mut errors: u64 = 0;
    let feeds_count = feeds.len() as u64;

    for feed in &feeds {
        match refresh_single_feed(&state, feed).await {
            Ok(count) => total_new += count,
            Err(e) => {
                errors += 1;
                tracing::warn!(feed_id = %feed.id, error = %e, "feed sync error");
                let db = state
                    .db
                    .lock()
                    .map_err(|_| ApiError::internal("database lock poisoned"))?;
                let _ = db.update_feed_fetch_error(feed.id, &e.to_string());
            }
        }
    }

    Ok(Json(SyncResponse {
        feeds_synced: feeds_count,
        new_items: total_new,
        errors,
    }))
}

/// `POST /api/feeds/import-opml` — import OPML from request body.
pub async fn import_opml(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<OpmlImportResponse>, ApiError> {
    let doc = pergamon_feed::parse_opml(&body)
        .map_err(|e| ApiError::unprocessable(format!("failed to parse OPML: {e}")))?;

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    let mut import_result = OpmlImportResponse {
        folders_created: 0,
        folders_existing: 0,
        feeds_added: 0,
        feeds_existing: 0,
        feeds_moved: 0,
    };

    import_outlines(&db, &doc.outlines, None, &mut import_result)?;

    Ok(Json(import_result))
}

// ======================================================================
// Internal helpers
// ======================================================================

/// Refresh a single feed: conditional GET → parse → ingest.
async fn refresh_single_feed(state: &AppState, feed: &Feed) -> Result<u64, ApiError> {
    let mut req = state.http.get(&feed.url);

    // Conditional GET headers.
    if let Some(etag) = &feed.etag {
        req = req.header("If-None-Match", etag.as_str());
    }
    if let Some(lm) = &feed.last_modified_header {
        req = req.header("If-Modified-Since", lm.as_str());
    }

    let response = req
        .send()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("failed to fetch {}: {e}", feed.url)))?;

    // 304 Not Modified.
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        let db = state
            .db
            .lock()
            .map_err(|_| ApiError::internal("database lock poisoned"))?;
        db.update_feed_fetch_success(
            feed.id,
            feed.etag.as_deref(),
            feed.last_modified_header.as_deref(),
        )?;
        return Ok(0);
    }

    if !response.status().is_success() {
        return Err(ApiError::bad_gateway(format!(
            "HTTP {} for {}",
            response.status(),
            feed.url
        )));
    }

    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("failed to read response: {e}")))?;

    let parsed = pergamon_feed::parse_feed(&bytes, &feed.url)
        .map_err(|e| ApiError::unprocessable(format!("failed to parse feed: {e}")))?;

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    let count = ingest_entries(&db, feed, &parsed.entries)?;
    db.update_feed_fetch_success(feed.id, etag.as_deref(), last_modified.as_deref())?;

    Ok(count)
}

/// Ingest parsed feed entries, skipping duplicates.
fn ingest_entries(
    db: &pergamon_storage::Database,
    feed: &Feed,
    entries: &[pergamon_feed::ParsedEntry],
) -> Result<u64, ApiError> {
    let mut count: u64 = 0;

    for entry in entries {
        let is_dup = if let Some(guid) = &entry.guid {
            db.feed_item_exists_by_guid(feed.id, guid)?
        } else if let Some(url) = &entry.url {
            db.feed_item_exists_by_url(feed.id, url)?
        } else {
            false
        };

        if is_dup {
            continue;
        }

        let now = OffsetDateTime::now_utc();
        let item = ContentItem {
            id: Uuid::new_v4(),
            url: entry.url.clone(),
            title: entry.title.clone(),
            author: entry.author.clone(),
            content_type: ContentType::FeedItem,
            status: DocumentStatus::Inbox,
            content_text: entry.content.clone(),
            excerpt: entry.summary.clone(),
            published_at: entry.published_at,
            created_at: now,
            updated_at: now,
            read_at: None,
        };

        db.insert_content_item(&item)?;

        let meta = FeedItemMeta {
            content_item_id: item.id,
            feed_id: feed.id,
            guid: entry.guid.clone(),
            summary: entry.summary.clone(),
        };
        db.insert_feed_item_meta(&meta)?;
        count += 1;
    }

    Ok(count)
}

/// Recursively import OPML outlines into the database.
fn import_outlines(
    db: &pergamon_storage::Database,
    outlines: &[pergamon_feed::OpmlOutline],
    parent_folder_id: Option<Uuid>,
    stats: &mut OpmlImportResponse,
) -> Result<(), ApiError> {
    for outline in outlines {
        if outline.is_feed() {
            let Some(xml_url) = outline.xml_url.as_deref() else {
                continue;
            };
            let title = outline.display_name();

            let existing = db.get_feed_by_url(xml_url)?;

            if let Some(existing_feed) = existing {
                if existing_feed.folder_id == parent_folder_id {
                    stats.feeds_existing += 1;
                } else {
                    db.update_feed_folder_id(existing_feed.id, parent_folder_id)?;
                    stats.feeds_moved += 1;
                }
            } else {
                let now = OffsetDateTime::now_utc();
                let feed = Feed {
                    id: Uuid::new_v4(),
                    title: title.to_owned(),
                    url: xml_url.to_owned(),
                    site_url: outline.html_url.clone(),
                    description: None,
                    etag: None,
                    last_modified_header: None,
                    error_count: 0,
                    last_error: None,
                    last_fetched_at: None,
                    folder_id: parent_folder_id,
                    created_at: now,
                    updated_at: now,
                };
                db.insert_feed(&feed)?;
                stats.feeds_added += 1;
            }
        } else {
            let folder_name = outline.display_name();

            if folder_name.is_empty() {
                import_outlines(db, &outline.children, parent_folder_id, stats)?;
                continue;
            }

            let folder_id = get_or_create_folder(db, folder_name, parent_folder_id)?;

            if let Some(existing) = db.get_feed_folder_by_name(folder_name, parent_folder_id)? {
                if existing.id == folder_id {
                    stats.folders_existing += 1;
                }
            }

            import_outlines(db, &outline.children, Some(folder_id), stats)?;
        }
    }
    Ok(())
}

/// Get or create a folder by name and parent.
fn get_or_create_folder(
    db: &pergamon_storage::Database,
    name: &str,
    parent_id: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    if let Some(existing) = db.get_feed_folder_by_name(name, parent_id)? {
        return Ok(existing.id);
    }

    let now = OffsetDateTime::now_utc();
    let folder = FeedFolder {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        parent_id,
        created_at: now,
        updated_at: now,
    };
    db.insert_feed_folder(&folder)?;
    Ok(folder.id)
}
