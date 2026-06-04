// SPDX-License-Identifier: AGPL-3.0-only

//! Tag management API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use pergamon_core::model::Tag;

use crate::error::ApiError;
use crate::state::AppState;

// ======================================================================
// Request / response types
// ======================================================================

/// Response for listing tags with counts.
#[derive(Debug, Serialize)]
pub struct TagWithCount {
    /// Tag name.
    pub name: String,
    /// Number of items with this tag.
    pub count: i64,
}

/// Request body for tagging an item.
#[derive(Debug, Deserialize)]
pub struct AddTagsRequest {
    /// Single tag name (use `name` or `names`, not both).
    pub name: Option<String>,
    /// Multiple tag names.
    pub names: Option<Vec<String>>,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/tags` — list all tags with item counts.
pub async fn list_tags(State(state): State<AppState>) -> Result<Json<Vec<TagWithCount>>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Get tags with counts (only tags that have items).
    let tag_counts = db.list_tags_with_counts()?;
    let with_counts: Vec<TagWithCount> = tag_counts
        .into_iter()
        .map(|tc| TagWithCount {
            name: tc.tag_name,
            count: tc.count,
        })
        .collect();

    // Also include zero-count tags from the full tag list.
    let all_tags = db.list_tags()?;
    let mut result = with_counts;
    for tag in all_tags {
        if !result.iter().any(|t| t.name == tag.name) {
            result.push(TagWithCount {
                name: tag.name,
                count: 0,
            });
        }
    }

    Ok(Json(result))
}

/// `POST /api/items/{id}/tags` — add tags to a content item.
pub async fn add_tags(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Json(body): Json<AddTagsRequest>,
) -> Result<Json<Vec<Tag>>, ApiError> {
    let tag_names = resolve_tag_names(&body)?;

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify item exists.
    db.get_content_item(item_id)
        .map_err(|_| ApiError::not_found("item not found"))?;

    let mut applied_tags = Vec::new();
    for name in &tag_names {
        let tag = db.get_or_create_tag(name)?;
        let _ = db.tag_content_item(item_id, tag.id);
        applied_tags.push(tag);
    }

    Ok(Json(applied_tags))
}

/// `DELETE /api/items/{id}/tags/{tag}` — remove a tag from a content item.
pub async fn remove_tag(
    State(state): State<AppState>,
    Path((item_id, tag_name)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify item exists.
    db.get_content_item(item_id)
        .map_err(|_| ApiError::not_found("item not found"))?;

    // Find the tag by name.
    let tag = db
        .get_tag_by_name(&tag_name)?
        .ok_or_else(|| ApiError::not_found(format!("tag '{tag_name}' not found")))?;

    let removed = db.untag_content_item(item_id, tag.id)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("tag was not applied to this item"))
    }
}

// ======================================================================
// Helpers
// ======================================================================

/// Resolve tag names from the request body (supports `name` or `names`).
fn resolve_tag_names(body: &AddTagsRequest) -> Result<Vec<String>, ApiError> {
    match (&body.name, &body.names) {
        (Some(name), None) => Ok(vec![name.clone()]),
        (None, Some(names)) => {
            if names.is_empty() {
                return Err(ApiError::bad_request("names array must not be empty"));
            }
            Ok(names.clone())
        }
        (Some(_), Some(_)) => Err(ApiError::bad_request(
            "provide either 'name' or 'names', not both",
        )),
        (None, None) => Err(ApiError::bad_request("provide 'name' or 'names'")),
    }
}
