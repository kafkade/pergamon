// SPDX-License-Identifier: AGPL-3.0-only

//! Server-rendered collections view: list regular and smart collections,
//! a detail view with member items, create/edit/delete, a smart-filter (DSL)
//! editor, and drag-and-drop member reordering.
//!
//! Reordering is JavaScript-enhanced (drag-and-drop posts the new order) but
//! degrades to per-item "move up / move down" forms that work without JS, in
//! keeping with the progressive-enhancement approach used across [`super::web`].

#![allow(clippy::significant_drop_tightening)]

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use pergamon_core::model::Collection;
use pergamon_core::smart_filter::SmartFilter;

use super::web::{ItemView, build_item_view, internal_error, is_htmx, not_found, render};
use crate::state::AppState;

// ======================================================================
// View models
// ======================================================================

/// A collection summarised for the listing page.
struct CollectionRowView {
    name: String,
    filter_query: String,
    member_count: usize,
    detail_href: String,
    parent_name: String,
}

/// An option in the parent-collection dropdown.
struct ParentOption {
    id: String,
    name: String,
}

#[derive(Template)]
#[template(path = "collections.html")]
struct CollectionsTemplate {
    regular: Vec<CollectionRowView>,
    smart: Vec<CollectionRowView>,
    parents: Vec<ParentOption>,
}

/// A member item within a collection detail view.
struct MemberView {
    item: ItemView,
    remove_href: String,
    move_up_href: String,
    move_down_href: String,
    is_first: bool,
    is_last: bool,
}

#[derive(Template)]
#[template(path = "collection_detail.html")]
struct CollectionDetailTemplate {
    id: String,
    name: String,
    is_smart: bool,
    filter_query: String,
    member_count: usize,
    members: Vec<MemberView>,
    reorder_url: String,
}

// ======================================================================
// Form types
// ======================================================================

/// Form body for creating a collection.
#[derive(Debug, Deserialize)]
pub struct CreateCollectionForm {
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    filter_query: String,
    #[serde(default)]
    parent_id: String,
}

/// Form body for renaming a collection.
#[derive(Debug, Deserialize)]
pub struct RenameCollectionForm {
    name: String,
}

/// Form body for editing a smart collection's filter.
#[derive(Debug, Deserialize)]
pub struct UpdateFilterForm {
    filter_query: String,
}

/// Form body for moving a member up or down.
#[derive(Debug, Deserialize)]
pub struct MoveMemberForm {
    dir: String,
}

// ======================================================================
// Handlers
// ======================================================================

/// `GET /collections` — list regular and smart collections.
pub async fn collections(State(state): State<AppState>) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let Ok(all) = db.list_collections() else {
        return internal_error();
    };

    // Resolve parent names for display.
    let name_of = |id: Uuid| -> String {
        all.iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    };

    let mut regular = Vec::new();
    let mut smart = Vec::new();
    for coll in &all {
        let member_count = if coll.is_smart {
            db.count_smart_collection_items(coll.id).unwrap_or(0)
        } else {
            db.list_collection_items(coll.id).map_or(0, |v| v.len())
        };
        let row = CollectionRowView {
            name: coll.name.clone(),
            filter_query: coll.filter_query.clone().unwrap_or_default(),
            member_count,
            detail_href: format!("/collections/{}", coll.id),
            parent_name: coll.parent_id.map(name_of).unwrap_or_default(),
        };
        if coll.is_smart {
            smart.push(row);
        } else {
            regular.push(row);
        }
    }

    let parents = all
        .iter()
        .filter(|c| !c.is_smart)
        .map(|c| ParentOption {
            id: c.id.to_string(),
            name: c.name.clone(),
        })
        .collect();

    render(&CollectionsTemplate {
        regular,
        smart,
        parents,
    })
}

/// `GET /collections/{id}` — collection detail with member items.
pub async fn collection_detail(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    let Ok(coll) = db.get_collection(id) else {
        return not_found();
    };

    let items = if coll.is_smart {
        db.list_smart_collection_items(id).unwrap_or_default()
    } else {
        db.list_collection_items(id).unwrap_or_default()
    };

    let count = items.len();
    let members: Vec<MemberView> = items
        .iter()
        .enumerate()
        .map(|(idx, item)| MemberView {
            item: build_item_view(&db, item),
            remove_href: format!("/collections/{}/items/{}/remove", id, item.id),
            move_up_href: format!("/collections/{}/items/{}/move", id, item.id),
            move_down_href: format!("/collections/{}/items/{}/move", id, item.id),
            is_first: idx == 0,
            is_last: idx + 1 == count,
        })
        .collect();

    render(&CollectionDetailTemplate {
        id: id.to_string(),
        name: coll.name,
        is_smart: coll.is_smart,
        filter_query: coll.filter_query.unwrap_or_default(),
        member_count: count,
        members,
        reorder_url: format!("/collections/{id}/reorder"),
    })
}

/// `POST /collections/create` — create a regular or smart collection.
pub async fn create_collection(
    State(state): State<AppState>,
    Form(form): Form<CreateCollectionForm>,
) -> Response {
    let name = form.name.trim();
    if name.is_empty() {
        return Redirect::to("/collections").into_response();
    }

    let is_smart = form.kind == "smart";
    let filter_query = form.filter_query.trim();
    if is_smart && (filter_query.is_empty() || SmartFilter::parse(filter_query).is_err()) {
        return Redirect::to("/collections").into_response();
    }

    let parent_id = Uuid::parse_str(form.parent_id.trim()).ok();

    let Ok(db) = state.db.lock() else {
        return internal_error();
    };

    if matches!(db.get_collection_by_name(name), Ok(Some(_))) {
        return Redirect::to("/collections").into_response();
    }

    let now = OffsetDateTime::now_utc();
    let collection = Collection {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        parent_id,
        sort_order: 0,
        is_smart,
        filter_query: if is_smart {
            Some(filter_query.to_owned())
        } else {
            None
        },
        created_at: now,
        updated_at: now,
    };
    if db.insert_collection(&collection).is_err() {
        return internal_error();
    }

    Redirect::to(&format!("/collections/{}", collection.id)).into_response()
}

/// `POST /collections/{id}/rename` — rename a collection.
pub async fn rename_collection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Form(form): Form<RenameCollectionForm>,
) -> Response {
    let name = form.name.trim();
    if !name.is_empty() {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        let _ = db.rename_collection(id, name);
    }
    Redirect::to(&format!("/collections/{id}")).into_response()
}

/// `POST /collections/{id}/filter` — update a smart collection's filter.
pub async fn update_filter(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Form(form): Form<UpdateFilterForm>,
) -> Response {
    let filter_query = form.filter_query.trim();
    if !filter_query.is_empty() && SmartFilter::parse(filter_query).is_ok() {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        let _ = db.update_smart_filter(id, filter_query);
    }
    Redirect::to(&format!("/collections/{id}")).into_response()
}

/// `POST /collections/{id}/delete` — delete a collection.
pub async fn delete_collection(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        let _ = db.delete_collection(id);
    }
    Redirect::to("/collections").into_response()
}

/// `POST /collections/{id}/items/{item_id}/remove` — remove a member item.
pub async fn remove_item(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(Uuid, Uuid)>,
) -> Response {
    {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        let _ = db.remove_from_collection(item_id, id);
    }
    Redirect::to(&format!("/collections/{id}")).into_response()
}

/// `POST /collections/{id}/items/{item_id}/move` — move a member up or down.
///
/// A no-JS fallback for drag-and-drop reordering: computes the current order,
/// swaps the item with its neighbour, and persists the new order.
pub async fn move_item(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(Uuid, Uuid)>,
    Form(form): Form<MoveMemberForm>,
) -> Response {
    {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        if let Ok(items) = db.list_collection_items(id) {
            let mut ids: Vec<Uuid> = items.into_iter().map(|i| i.id).collect();
            if let Some(pos) = ids.iter().position(|x| *x == item_id) {
                let swap_with = match form.dir.as_str() {
                    "up" if pos > 0 => Some(pos - 1),
                    "down" if pos + 1 < ids.len() => Some(pos + 1),
                    _ => None,
                };
                if let Some(target) = swap_with {
                    ids.swap(pos, target);
                    let _ = db.reorder_collection_items(id, &ids);
                }
            }
        }
    }
    Redirect::to(&format!("/collections/{id}")).into_response()
}

/// `POST /collections/{id}/reorder` — set a new member order.
///
/// The body is parsed manually because `axum::Form` can't deserialize the
/// repeated `ids` field into a `Vec`. Used by the drag-and-drop enhancement,
/// which posts the full ordered list of item IDs.
pub async fn reorder(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ids: Vec<Uuid> = url::form_urlencoded::parse(&body)
        .filter(|(k, _)| k == "ids")
        .filter_map(|(_, v)| v.parse::<Uuid>().ok())
        .collect();

    {
        let Ok(db) = state.db.lock() else {
            return internal_error();
        };
        if let Err(e) = db.reorder_collection_items(id, &ids) {
            tracing::warn!(error = %e, "reorder failed");
        }
    }

    if is_htmx(&headers) {
        // Re-render the detail page's member list is overkill; a 204 keeps the
        // DOM the drag-and-drop script already updated.
        return StatusCode::NO_CONTENT.into_response();
    }
    Redirect::to(&format!("/collections/{id}")).into_response()
}

// ======================================================================
// Helpers
// ======================================================================
