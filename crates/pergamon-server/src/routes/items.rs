// SPDX-License-Identifier: AGPL-3.0-only

//! Content item API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::model::{BookmarkMeta, ContentItem, Tag};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::{ContentItemFilter, SearchFilter};

use crate::error::ApiError;
use crate::pagination::{Paginated, PaginationParams};
use crate::state::AppState;

// ======================================================================
// Query / request / response types
// ======================================================================

/// Query parameters for listing content items.
#[derive(Debug, Deserialize)]
pub struct ListItemsQuery {
    /// Filter by document status.
    pub status: Option<DocumentStatus>,
    /// Filter by content type.
    pub content_type: Option<ContentType>,
    /// Filter by tag name.
    pub tag: Option<String>,
    /// Filter by feed ID.
    pub feed_id: Option<Uuid>,
    /// Filter by folder ID.
    pub folder_id: Option<Uuid>,
    /// Full-text search query.
    pub search: Option<String>,
    /// Page number (1-indexed).
    pub page: Option<u32>,
    /// Items per page.
    pub per_page: Option<u32>,
}

/// Request body for saving a URL.
#[derive(Debug, Deserialize)]
pub struct SaveItemRequest {
    /// URL to save.
    pub url: String,
    /// Tags to apply.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Save as bookmark without article extraction.
    #[serde(default)]
    pub bookmark: bool,
}

/// Request body for updating an item.
#[derive(Debug, Deserialize)]
pub struct UpdateItemRequest {
    /// New status.
    pub status: Option<DocumentStatus>,
    /// Tags to set (replaces existing tags).
    pub tags: Option<Vec<String>>,
}

/// Response for a single item with its tags.
#[derive(Debug, Serialize)]
pub struct ItemResponse {
    /// The content item.
    #[serde(flatten)]
    pub item: ContentItem,
    /// Tags applied to this item.
    pub tags: Vec<Tag>,
}

/// Response for save operation.
#[derive(Debug, Serialize)]
pub struct SaveResponse {
    /// The saved content item.
    #[serde(flatten)]
    pub item: ContentItem,
    /// Whether this was a duplicate.
    pub duplicate: bool,
    /// Tags that were applied.
    pub tags_applied: Vec<String>,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/items` — list content items with filters and pagination.
pub async fn list_items(
    State(state): State<AppState>,
    Query(query): Query<ListItemsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let pagination = PaginationParams {
        page: query.page,
        per_page: query.per_page,
    };

    // If a search query is present, use FTS search.
    if let Some(ref search_query) = query.search {
        let db = state
            .db
            .lock()
            .map_err(|_| ApiError::internal("database lock poisoned"))?;
        let filter = SearchFilter {
            content_type: query.content_type,
            status: query.status,
            tag_name: query.tag.clone(),
            feed_id: query.feed_id,
            since: None,
            before: None,
        };
        // Search does not support offset; return limited results.
        let hits = db.search_filtered(search_query, &filter, Some(pagination.limit()))?;
        return Ok(Json(hits).into_response());
    }

    // Regular filtered listing with pagination.
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    let tag_id = if let Some(ref tag_name) = query.tag {
        let tag = db.get_tag_by_name(tag_name)?;
        tag.map(|t| t.id)
    } else {
        None
    };

    let filter = ContentItemFilter {
        content_type: query.content_type,
        status: query.status,
        feed_id: query.feed_id,
        folder_id: query.folder_id,
        tag_id,
        ..ContentItemFilter::default()
    };

    let total = db.count_content_items_filtered(&filter)?;
    let items = db.list_content_items_filtered(
        &filter,
        Some(pagination.limit()),
        Some(pagination.offset()),
    )?;

    Ok(Paginated::new(items, total, &pagination, "/api/items").into_response())
}

/// `GET /api/items/{id}` — get a single item with its tags.
pub async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ItemResponse>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let item = db
        .get_content_item(id)
        .map_err(|_| ApiError::not_found("item not found"))?;
    let tags = db.tags_for_item(id)?;
    Ok(Json(ItemResponse { item, tags }))
}

/// `POST /api/items` — save a URL (fetch, extract, store).
pub async fn create_item(
    State(state): State<AppState>,
    Json(body): Json<SaveItemRequest>,
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

    // Fetch the page.
    let response = state
        .http
        .get(body.url.as_str())
        .send()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("failed to fetch URL: {e}")))?;

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
        .map_err(|e| ApiError::bad_gateway(format!("failed to read response: {e}")))?;

    // Limit response size (10 MB).
    if bytes.len() > 10 * 1024 * 1024 {
        return Err(ApiError::bad_request("response too large (max 10 MB)"));
    }

    // Canonicalize URL for dedup.
    let canonical_url =
        pergamon_extract::canonicalize_url(&final_url).unwrap_or_else(|_| final_url.clone());

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Check for duplicate.
    if let Some(existing) = db.get_content_item_by_url(&canonical_url)? {
        let tags_applied = apply_tags(&db, existing.id, &body.tags)?;

        if body.bookmark {
            let meta = build_bookmark_meta(existing.id, &bytes, &body.url, &final_url);
            let _ = db.upsert_bookmark_meta(&meta);
        }

        return Ok((
            StatusCode::OK,
            Json(SaveResponse {
                item: existing,
                duplicate: true,
                tags_applied,
            }),
        )
            .into_response());
    }

    // Extract content.
    let now = OffsetDateTime::now_utc();
    let (title, author, content_text, excerpt, published_at, content_type) =
        extract_content(&bytes, &final_url, &canonical_url, body.bookmark);

    let item = ContentItem {
        id: Uuid::new_v4(),
        url: Some(canonical_url),
        title,
        author,
        content_type,
        status: DocumentStatus::Inbox,
        content_text,
        excerpt,
        published_at,
        created_at: now,
        updated_at: now,
        read_at: None,
    };

    db.insert_content_item(&item)?;

    let meta = build_bookmark_meta(item.id, &bytes, &body.url, &final_url);
    let _ = db.insert_bookmark_meta(&meta);

    let tags_applied = apply_tags(&db, item.id, &body.tags)?;

    Ok((
        StatusCode::CREATED,
        Json(SaveResponse {
            item,
            duplicate: false,
            tags_applied,
        }),
    )
        .into_response())
}

/// `PATCH /api/items/{id}` — update an item's status or tags.
pub async fn update_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateItemRequest>,
) -> Result<Json<ItemResponse>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify item exists.
    let item = db
        .get_content_item(id)
        .map_err(|_| ApiError::not_found("item not found"))?;

    // Update status if provided.
    if let Some(new_status) = body.status {
        db.update_content_item_status(id, new_status)?;
    }

    // Update tags if provided (replace all).
    if let Some(ref tag_names) = body.tags {
        // Remove existing tags.
        let existing_tags = db.tags_for_item(id)?;
        for tag in &existing_tags {
            let _ = db.untag_content_item(id, tag.id);
        }
        // Apply new tags.
        for name in tag_names {
            let tag = db.get_or_create_tag(name)?;
            let _ = db.tag_content_item(id, tag.id);
        }
    }

    // Re-fetch the item to reflect updates.
    let updated_item = db.get_content_item(id).unwrap_or(item);
    let tags = db.tags_for_item(id)?;

    Ok(Json(ItemResponse {
        item: updated_item,
        tags,
    }))
}

/// `DELETE /api/items/{id}` — delete a content item.
pub async fn delete_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let deleted = db.delete_content_item(id)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("item not found"))
    }
}

// ======================================================================
// Helpers
// ======================================================================

/// Apply tags to a content item, returning the names of tags applied.
fn apply_tags(
    db: &pergamon_storage::Database,
    item_id: Uuid,
    tag_names: &[String],
) -> Result<Vec<String>, ApiError> {
    let mut applied = Vec::new();
    for name in tag_names {
        let tag = db.get_or_create_tag(name)?;
        db.tag_content_item(item_id, tag.id)?;
        applied.push(tag.name);
    }
    Ok(applied)
}

/// Extract content from fetched HTML bytes.
///
/// Replicates the extraction logic from `pergamon-cli`.
fn extract_content(
    bytes: &[u8],
    final_url: &str,
    canonical_url: &str,
    bookmark: bool,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<OffsetDateTime>,
    ContentType,
) {
    if bookmark {
        let html = String::from_utf8_lossy(bytes);
        let meta = pergamon_extract::extract_metadata(&html);
        (
            meta.title.unwrap_or_else(|| canonical_url.to_owned()),
            meta.author,
            None,
            meta.description,
            None,
            ContentType::Bookmark,
        )
    } else if let Ok(article) = pergamon_extract::extract_article(bytes, final_url) {
        (
            article.title.unwrap_or_else(|| canonical_url.to_owned()),
            article.author,
            Some(article.content_text),
            article.excerpt,
            article.published_at,
            ContentType::Article,
        )
    } else {
        let html = String::from_utf8_lossy(bytes);
        let meta = pergamon_extract::extract_metadata(&html);
        (
            meta.title.unwrap_or_else(|| canonical_url.to_owned()),
            meta.author,
            None,
            meta.description,
            None,
            ContentType::Bookmark,
        )
    }
}

/// Build enriched `BookmarkMeta` from HTML bytes.
fn build_bookmark_meta(
    content_item_id: Uuid,
    bytes: &[u8],
    original_url: &str,
    final_url: &str,
) -> BookmarkMeta {
    let html = String::from_utf8_lossy(bytes);
    let meta = pergamon_extract::extract_metadata(&html);

    let favicon_url = meta
        .favicon_url
        .and_then(|href| pergamon_extract::resolve_favicon_url(&href, final_url));

    BookmarkMeta {
        content_item_id,
        original_url: Some(original_url.to_owned()),
        saved_from: Some("web".to_owned()),
        thumbnail_url: meta.og_image,
        description: meta.description,
        site_name: meta.site_name,
        favicon_url,
    }
}
