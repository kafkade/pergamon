// SPDX-License-Identifier: AGPL-3.0-only

//! Server-rendered tag management view: a tag cloud/list with item counts and
//! rename / merge / delete operations.
//!
//! Tag-management forms are plain `POST` submissions that redirect back to the
//! page, so the view works fully without JavaScript (see [`super::web`]).

#![allow(clippy::significant_drop_tightening)]

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::web::{internal_error, render};
use crate::state::AppState;

// ======================================================================
// View models
// ======================================================================

/// A tag in the cloud/list with its count and a relative weight bucket (1-5).
struct TagView {
    name: String,
    count: i64,
    weight: u8,
    filter_href: String,
}

#[derive(Template)]
#[template(path = "tags.html")]
struct TagsTemplate {
    tags: Vec<TagView>,
    total: usize,
}

// ======================================================================
// Form types
// ======================================================================

/// Form body for renaming a tag.
#[derive(Debug, Deserialize)]
pub struct RenameTagForm {
    new_name: String,
}

/// Form body for merging a tag into another.
#[derive(Debug, Deserialize)]
pub struct MergeTagForm {
    target: String,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /tags` — tag cloud/list with item counts.
pub async fn tags(State(state): State<AppState>) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    // Tags with counts, plus any zero-count tags from the full list.
    let Ok(counted) = db.list_tags_with_counts() else {
        return internal_error();
    };
    let mut tag_views: Vec<TagView> = Vec::new();
    let max = counted.iter().map(|t| t.count).max().unwrap_or(0);

    for tc in &counted {
        tag_views.push(TagView {
            weight: weight_for(tc.count, max),
            filter_href: format!("/inbox?tag={}", super::web::urlencode(&tc.tag_name)),
            name: tc.tag_name.clone(),
            count: tc.count,
        });
    }

    if let Ok(all) = db.list_tags() {
        for tag in all {
            if !tag_views.iter().any(|t| t.name == tag.name) {
                tag_views.push(TagView {
                    weight: 1,
                    filter_href: format!("/inbox?tag={}", super::web::urlencode(&tag.name)),
                    name: tag.name,
                    count: 0,
                });
            }
        }
    }

    tag_views.sort_by_key(|a| a.name.to_lowercase());
    let total = tag_views.len();

    render(&TagsTemplate {
        tags: tag_views,
        total,
    })
}

/// `POST /tags/{name}/rename` — rename a tag (merges if the target exists).
pub async fn rename_tag(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Form(form): Form<RenameTagForm>,
) -> Response {
    let new_name = form.new_name.trim();
    if new_name.is_empty() {
        return Redirect::to("/tags").into_response();
    }

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let Ok(Some(tag)) = db.get_tag_by_name(&name) else {
        return Redirect::to("/tags").into_response();
    };

    // If the new name already belongs to a *different* tag, merge into it;
    // otherwise it's a plain rename.
    match db.get_tag_by_name(new_name) {
        Ok(Some(target)) if target.id != tag.id => {
            let _ = db.merge_tags(tag.id, target.id);
        }
        Ok(_) => {
            let _ = db.rename_tag(tag.id, new_name);
        }
        Err(_) => return internal_error(),
    }

    Redirect::to("/tags").into_response()
}

/// `POST /tags/{name}/merge` — merge a tag into a target tag.
pub async fn merge_tag(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Form(form): Form<MergeTagForm>,
) -> Response {
    let target_name = form.target.trim();
    if target_name.is_empty() {
        return Redirect::to("/tags").into_response();
    }

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let Ok(Some(source)) = db.get_tag_by_name(&name) else {
        return Redirect::to("/tags").into_response();
    };
    let Ok(target) = db.get_or_create_tag(target_name) else {
        return internal_error();
    };
    let _ = db.merge_tags(source.id, target.id);

    Redirect::to("/tags").into_response()
}

/// `POST /tags/{name}/delete` — delete a tag.
pub async fn delete_tag(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };
    if let Ok(Some(tag)) = db.get_tag_by_name(&name) {
        let _ = db.delete_tag(tag.id);
    }
    Redirect::to("/tags").into_response()
}

// ======================================================================
// Helpers
// ======================================================================

/// Bucket a tag count into a 1-5 weight for cloud sizing.
fn weight_for(count: i64, max: i64) -> u8 {
    if max <= 0 || count <= 0 {
        return 1;
    }
    // Linear bucket across 1..=5 using integer math (rounded division).
    let bucket = 1 + (count * 4 + max / 2) / max;
    u8::try_from(bucket.clamp(1, 5)).unwrap_or(1)
}
