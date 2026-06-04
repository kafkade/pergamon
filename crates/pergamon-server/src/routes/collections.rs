// SPDX-License-Identifier: AGPL-3.0-only

//! Collection management API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::model::{Collection, ContentItem};

use crate::error::ApiError;
use crate::state::AppState;

// ======================================================================
// Request types
// ======================================================================

/// Request body for creating a collection.
#[derive(Debug, Deserialize)]
pub struct CreateCollectionRequest {
    /// Collection name.
    pub name: String,
    /// Parent collection ID (for nesting).
    pub parent_id: Option<Uuid>,
    /// Whether this is a smart (auto-populated) collection.
    #[serde(default)]
    pub is_smart: bool,
    /// Filter query for smart collections.
    pub filter_query: Option<String>,
}

/// Request body for adding items to a collection.
#[derive(Debug, Deserialize)]
pub struct AddItemsRequest {
    /// Content item IDs to add.
    pub item_ids: Vec<Uuid>,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/collections` — list all collections.
pub async fn list_collections(
    State(state): State<AppState>,
) -> Result<Json<Vec<Collection>>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let collections = db.list_collections()?;
    Ok(Json(collections))
}

/// `POST /api/collections` — create a new collection.
pub async fn create_collection(
    State(state): State<AppState>,
    Json(body): Json<CreateCollectionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::bad_request("collection name must not be empty"));
    }

    if body.is_smart && body.filter_query.is_none() {
        return Err(ApiError::bad_request(
            "smart collections require a filter_query",
        ));
    }

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Check for duplicate name.
    if db.get_collection_by_name(&body.name)?.is_some() {
        return Err(ApiError::conflict(format!(
            "collection '{}' already exists",
            body.name
        )));
    }

    let now = OffsetDateTime::now_utc();
    let collection = Collection {
        id: Uuid::new_v4(),
        name: body.name,
        parent_id: body.parent_id,
        sort_order: 0,
        is_smart: body.is_smart,
        filter_query: body.filter_query,
        created_at: now,
        updated_at: now,
    };

    db.insert_collection(&collection)?;

    Ok((StatusCode::CREATED, Json(collection)))
}

/// `GET /api/collections/{id}/items` — list items in a collection.
pub async fn list_collection_items(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ContentItem>>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify collection exists.
    let collection = db
        .get_collection(id)
        .map_err(|_| ApiError::not_found("collection not found"))?;

    let items = if collection.is_smart {
        db.list_smart_collection_items(id)?
    } else {
        db.list_collection_items(id)?
    };

    Ok(Json(items))
}

/// `POST /api/collections/{id}/items` — add items to a collection.
pub async fn add_collection_items(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AddItemsRequest>,
) -> Result<StatusCode, ApiError> {
    if body.item_ids.is_empty() {
        return Err(ApiError::bad_request("item_ids must not be empty"));
    }

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify collection exists.
    let collection = db
        .get_collection(id)
        .map_err(|_| ApiError::not_found("collection not found"))?;

    if collection.is_smart {
        return Err(ApiError::bad_request(
            "cannot manually add items to a smart collection",
        ));
    }

    db.bulk_add_to_collection(&body.item_ids, id)?;

    Ok(StatusCode::NO_CONTENT)
}
