// SPDX-License-Identifier: AGPL-3.0-only

//! Full-text search and saved-search API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::model::{Collection, SearchHit};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::SearchFilter;

use crate::error::ApiError;
use crate::state::AppState;
use crate::util::parse_date_param;

// ======================================================================
// Query / request types
// ======================================================================

/// Query parameters for the search endpoint.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// Full-text query string.
    pub q: String,
    /// Filter by content type.
    #[serde(rename = "type")]
    pub content_type: Option<ContentType>,
    /// Filter by document status.
    pub status: Option<DocumentStatus>,
    /// Filter by tag name (case-insensitive).
    pub tag: Option<String>,
    /// Filter to a specific feed (source).
    pub feed_id: Option<Uuid>,
    /// Only include items created on or after this date (YYYY-MM-DD).
    pub since: Option<String>,
    /// Only include items created before this date (YYYY-MM-DD).
    pub before: Option<String>,
    /// Maximum number of results to return.
    pub limit: Option<u32>,
}

/// Request body for saving a search.
#[derive(Debug, Deserialize)]
pub struct SaveSearchRequest {
    /// Display name for the saved search.
    pub name: String,
    /// Smart-collection filter query (DSL syntax).
    pub filter_query: String,
}

/// Default and maximum result limits for search.
const DEFAULT_SEARCH_LIMIT: u32 = 50;
const MAX_SEARCH_LIMIT: u32 = 200;

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/search` — full-text search with faceted filters.
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    if query.q.trim().is_empty() {
        return Err(ApiError::bad_request(
            "query parameter 'q' must not be empty",
        ));
    }

    let since: Option<OffsetDateTime> = query.since.as_deref().map(parse_date_param).transpose()?;
    let before: Option<OffsetDateTime> =
        query.before.as_deref().map(parse_date_param).transpose()?;

    let limit = query
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .min(MAX_SEARCH_LIMIT);

    let filter = SearchFilter {
        content_type: query.content_type,
        status: query.status,
        tag_name: query.tag,
        feed_id: query.feed_id,
        since,
        before,
    };

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let hits = db.search_filtered(&query.q, &filter, Some(limit))?;
    Ok(Json(hits))
}

/// `GET /api/saved-searches` — list saved searches (smart collections).
pub async fn list_saved_searches(
    State(state): State<AppState>,
) -> Result<Json<Vec<Collection>>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let saved: Vec<Collection> = db
        .list_collections()?
        .into_iter()
        .filter(|c| c.is_smart)
        .collect();
    Ok(Json(saved))
}

/// `POST /api/saved-searches` — save a search as a smart collection.
pub async fn create_saved_search(
    State(state): State<AppState>,
    Json(body): Json<SaveSearchRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }
    if body.filter_query.trim().is_empty() {
        return Err(ApiError::bad_request("filter_query must not be empty"));
    }

    // Validate the filter DSL before persisting.
    pergamon_core::smart_filter::SmartFilter::parse(&body.filter_query)
        .map_err(|e| ApiError::bad_request(format!("invalid filter_query: {e}")))?;

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    if db.get_collection_by_name(&body.name)?.is_some() {
        return Err(ApiError::conflict(format!(
            "a collection named '{}' already exists",
            body.name
        )));
    }

    let now = OffsetDateTime::now_utc();
    let collection = Collection {
        id: Uuid::new_v4(),
        name: body.name,
        parent_id: None,
        sort_order: 0,
        is_smart: true,
        filter_query: Some(body.filter_query),
        created_at: now,
        updated_at: now,
    };

    db.insert_collection(&collection)?;

    Ok((StatusCode::CREATED, Json(collection)))
}
