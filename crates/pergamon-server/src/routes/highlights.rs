// SPDX-License-Identifier: AGPL-3.0-only

//! Highlight / annotation API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::content_type::ContentType;
use pergamon_core::model::{ContentItem, HighlightMeta};

use crate::error::ApiError;
use crate::state::AppState;
use crate::util::parse_date_param;

// ======================================================================
// Query / request / response types
// ======================================================================

/// Query parameters for listing highlights.
#[derive(Debug, Deserialize)]
pub struct ListHighlightsQuery {
    /// Filter to highlights taken from a specific source item.
    pub source_item_id: Option<Uuid>,
    /// Filter by tag name (case-insensitive).
    pub tag: Option<String>,
    /// Only include highlights created on or after this date (YYYY-MM-DD).
    pub since: Option<String>,
    /// Only include highlights created before this date (YYYY-MM-DD).
    pub before: Option<String>,
    /// Maximum number of results.
    pub limit: Option<u32>,
}

/// Request body for creating a highlight.
#[derive(Debug, Deserialize)]
pub struct CreateHighlightRequest {
    /// The highlighted (quoted) text.
    pub quote_text: String,
    /// Optional note attached to the highlight.
    pub note: Option<String>,
    /// Optional color label.
    pub color: Option<String>,
}

/// Request body for updating a highlight.
///
/// Only the fields present are updated (PATCH semantics). Use an explicit
/// JSON `null` to clear a field, which is why these are `Option<Option<_>>`:
/// the outer `Option` distinguishes "absent" from "present", and the inner
/// one carries the (possibly null) value.
#[derive(Debug, Deserialize)]
#[allow(clippy::option_option)]
pub struct UpdateHighlightRequest {
    /// New note text (`null` clears it).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub note: Option<Option<String>>,
    /// New color label (`null` clears it).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub color: Option<Option<String>>,
}

/// Distinguish "field absent" from "field present and null" for PATCH.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// A highlight paired with its source content item.
#[derive(Debug, Serialize)]
pub struct HighlightResponse {
    /// The highlight's own content item (type `highlight`).
    #[serde(flatten)]
    pub item: ContentItem,
    /// Highlight metadata (quote, note, color, offsets).
    pub highlight: HighlightMeta,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/highlights` — list highlights with filters.
pub async fn list_highlights(
    State(state): State<AppState>,
    Query(query): Query<ListHighlightsQuery>,
) -> Result<Json<Vec<HighlightResponse>>, ApiError> {
    let since: Option<OffsetDateTime> = query.since.as_deref().map(parse_date_param).transpose()?;
    let before: Option<OffsetDateTime> =
        query.before.as_deref().map(parse_date_param).transpose()?;

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let rows = db.list_highlights(
        query.source_item_id,
        query.tag.as_deref(),
        since,
        before,
        query.limit,
    )?;
    Ok(Json(to_responses(rows)))
}

/// `GET /api/items/{id}/highlights` — highlights for a specific source item.
pub async fn list_item_highlights(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<HighlightResponse>>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify the source item exists.
    db.get_content_item(id)
        .map_err(|_| ApiError::not_found("item not found"))?;

    let rows = db.list_highlights(Some(id), None, None, None, None)?;
    Ok(Json(to_responses(rows)))
}

/// `POST /api/items/{id}/highlights` — create a highlight on a source item.
pub async fn create_highlight(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateHighlightRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.quote_text.trim().is_empty() {
        return Err(ApiError::bad_request("quote_text must not be empty"));
    }

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify the source item exists.
    db.get_content_item(id)
        .map_err(|_| ApiError::not_found("item not found"))?;

    let item = db.create_highlight(
        id,
        &body.quote_text,
        body.note.as_deref(),
        body.color.as_deref(),
    )?;
    let highlight = db.get_highlight_meta(item.id)?;

    Ok((
        StatusCode::CREATED,
        Json(HighlightResponse { item, highlight }),
    ))
}

/// `PATCH /api/highlights/{id}` — update a highlight's note and/or color.
pub async fn update_highlight(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateHighlightRequest>,
) -> Result<Json<HighlightResponse>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Load existing metadata (404 if not a highlight).
    let existing = db
        .get_highlight_meta(id)
        .map_err(|_| ApiError::not_found("highlight not found"))?;

    // Merge provided fields over existing values (PATCH semantics).
    let note = body.note.unwrap_or(existing.note);
    let color = body.color.unwrap_or(existing.color);

    db.update_highlight_meta(id, note.as_deref(), color.as_deref())?;

    let item = db
        .get_content_item(id)
        .map_err(|_| ApiError::not_found("highlight not found"))?;
    let highlight = db.get_highlight_meta(id)?;

    Ok(Json(HighlightResponse { item, highlight }))
}

/// `DELETE /api/highlights/{id}` — delete a highlight.
pub async fn delete_highlight(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Ensure the target is actually a highlight before deleting.
    let item = db
        .get_content_item(id)
        .map_err(|_| ApiError::not_found("highlight not found"))?;
    if item.content_type != ContentType::Highlight {
        return Err(ApiError::not_found("highlight not found"));
    }

    let deleted = db.delete_content_item(id)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("highlight not found"))
    }
}

// ======================================================================
// Helpers
// ======================================================================

/// Convert storage `(ContentItem, HighlightMeta)` pairs into responses.
fn to_responses(rows: Vec<(ContentItem, HighlightMeta)>) -> Vec<HighlightResponse> {
    rows.into_iter()
        .map(|(item, highlight)| HighlightResponse { item, highlight })
        .collect()
}
