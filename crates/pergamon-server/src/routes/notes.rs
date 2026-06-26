// SPDX-License-Identifier: AGPL-3.0-only

//! Note API endpoints.
#![allow(clippy::significant_drop_tightening)]

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::model::Note;

use crate::error::ApiError;
use crate::state::AppState;

// ======================================================================
// Request types
// ======================================================================

/// Request body for creating a note.
#[derive(Debug, Deserialize)]
pub struct CreateNoteRequest {
    /// Free-form note body.
    pub body: String,
}

/// Request body for updating a note.
#[derive(Debug, Deserialize)]
pub struct UpdateNoteRequest {
    /// New note body.
    pub body: String,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /api/items/{id}/notes` — list notes for a content item.
pub async fn list_item_notes(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Note>>, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify the item exists.
    db.get_content_item(id)
        .map_err(|_| ApiError::not_found("item not found"))?;

    let notes = db.list_notes_for_item(id)?;
    Ok(Json(notes))
}

/// `POST /api/items/{id}/notes` — add a note to a content item.
pub async fn create_note(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateNoteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.body.trim().is_empty() {
        return Err(ApiError::bad_request("note body must not be empty"));
    }

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify the item exists.
    db.get_content_item(id)
        .map_err(|_| ApiError::not_found("item not found"))?;

    let now = OffsetDateTime::now_utc();
    let note = Note {
        id: Uuid::new_v4(),
        content_item_id: id,
        body: body.body,
        created_at: now,
        updated_at: now,
    };
    db.insert_note(&note)?;

    Ok((StatusCode::CREATED, Json(note)))
}

/// `PATCH /api/notes/{id}` — update a note's body.
pub async fn update_note(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateNoteRequest>,
) -> Result<Json<Note>, ApiError> {
    if body.body.trim().is_empty() {
        return Err(ApiError::bad_request("note body must not be empty"));
    }

    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;

    // Verify the note exists.
    db.get_note(id)
        .map_err(|_| ApiError::not_found("note not found"))?;

    db.update_note(id, &body.body)?;

    let updated = db.get_note(id)?;
    Ok(Json(updated))
}

/// `DELETE /api/notes/{id}` — delete a note.
pub async fn delete_note(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    let deleted = db.delete_note(id)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("note not found"))
    }
}
