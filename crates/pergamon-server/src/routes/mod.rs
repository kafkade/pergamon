// SPDX-License-Identifier: AGPL-3.0-only

//! Route assembly for the pergamon REST API.

pub mod admin;
pub mod collections;
pub mod feeds;
pub mod health;
pub mod highlights;
pub mod items;
pub mod notes;
pub mod review;
pub mod search;
pub mod static_assets;
pub mod stats;
pub mod tags;
pub mod web;
pub mod web_bookmarks;
pub mod web_collections;
pub mod web_search;
pub mod web_tags;

use axum::Router;
use axum::routing::{delete, get, patch, post};

use crate::state::AppState;

/// Build the admin diagnostics sub-router, gated by Basic auth.
///
/// The auth middleware is applied with `route_layer` so it runs only for these
/// admin routes, not for the rest of the application or the 404 fallback. The
/// middleware reads [`AppState`] via `from_fn_with_state`, so the caller passes
/// the shared state in.
pub fn admin_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin", get(admin::dashboard))
        .route("/admin/sync", post(admin::sync_all))
        .route("/admin/sync/{id}", post(admin::sync_one))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            crate::auth::require_admin_auth,
        ))
}

/// Build the complete application router.
///
/// Mounts all API endpoints under `/api/`, the server-rendered HTML views at
/// the root, and the health check at `/health`. Static file serving is added
/// separately in `main.rs` (embedded assets, or a disk directory override).
#[allow(clippy::too_many_lines)]
pub fn api_router() -> Router<AppState> {
    Router::new()
        // Health
        .route("/health", get(health::health))
        // Server-rendered web UI
        .route("/", get(web::index))
        .route("/inbox", get(web::inbox))
        .route("/search", get(web_search::search))
        .route("/search/save", post(web_search::save_search))
        .route(
            "/bookmarks",
            get(web_bookmarks::bookmarks).post(web_bookmarks::add_bookmark),
        )
        .route("/tags", get(web_tags::tags))
        .route("/tags/{name}/rename", post(web_tags::rename_tag))
        .route("/tags/{name}/merge", post(web_tags::merge_tag))
        .route("/tags/{name}/delete", post(web_tags::delete_tag))
        .route("/collections", get(web_collections::collections))
        .route(
            "/collections/create",
            post(web_collections::create_collection),
        )
        .route("/collections/{id}", get(web_collections::collection_detail))
        .route(
            "/collections/{id}/rename",
            post(web_collections::rename_collection),
        )
        .route(
            "/collections/{id}/filter",
            post(web_collections::update_filter),
        )
        .route(
            "/collections/{id}/delete",
            post(web_collections::delete_collection),
        )
        .route("/collections/{id}/reorder", post(web_collections::reorder))
        .route(
            "/collections/{id}/items/{item_id}/remove",
            post(web_collections::remove_item),
        )
        .route(
            "/collections/{id}/items/{item_id}/move",
            post(web_collections::move_item),
        )
        .route("/highlights", get(web::highlights))
        .route("/highlights/export", get(web::highlights_export))
        .route("/highlights/{id}/note", post(web::update_highlight_note))
        .route("/notes", get(web::notes))
        .route("/notes/create", post(web::create_note_web))
        .route("/notes/{id}/update", post(web::update_note_web))
        .route("/notes/{id}/delete", post(web::delete_note_web))
        .route("/review", get(web::review))
        .route("/review/{card_id}", post(web::submit_review_web))
        .route("/review/stats", get(web::review_stats_page))
        .route("/items/bulk", post(web::bulk))
        .route("/items/{id}", get(web::reader))
        .route("/items/{id}/status", post(web::item_status))
        .route("/items/{id}/tags", post(web::add_tag))
        .route("/items/{id}/tags/{tag}/delete", post(web::remove_tag))
        // Content items
        .route(
            "/api/items",
            get(items::list_items).post(items::create_item),
        )
        .route(
            "/api/items/{id}",
            get(items::get_item)
                .patch(items::update_item)
                .delete(items::delete_item),
        )
        // Feeds
        .route(
            "/api/feeds",
            get(feeds::list_feeds).post(feeds::subscribe_feed),
        )
        .route("/api/feeds/{id}", delete(feeds::delete_feed))
        .route("/api/feeds/sync", post(feeds::sync_feeds))
        .route("/api/feeds/import-opml", post(feeds::import_opml))
        // Tags
        .route("/api/tags", get(tags::list_tags))
        .route("/api/items/{id}/tags", post(tags::add_tags))
        .route("/api/items/{id}/tags/{tag}", delete(tags::remove_tag))
        // Collections
        .route(
            "/api/collections",
            get(collections::list_collections).post(collections::create_collection),
        )
        .route(
            "/api/collections/{id}/items",
            get(collections::list_collection_items).post(collections::add_collection_items),
        )
        // Search
        .route("/api/search", get(search::search))
        .route(
            "/api/saved-searches",
            get(search::list_saved_searches).post(search::create_saved_search),
        )
        // Highlights
        .route("/api/highlights", get(highlights::list_highlights))
        .route(
            "/api/items/{id}/highlights",
            get(highlights::list_item_highlights).post(highlights::create_highlight),
        )
        .route(
            "/api/highlights/{id}",
            patch(highlights::update_highlight).delete(highlights::delete_highlight),
        )
        // Notes
        .route(
            "/api/items/{id}/notes",
            get(notes::list_item_notes).post(notes::create_note),
        )
        .route(
            "/api/notes/{id}",
            patch(notes::update_note).delete(notes::delete_note),
        )
        // Review (FSRS)
        .route("/api/review/queue", get(review::review_queue))
        .route("/api/review/{card_id}", post(review::submit_review))
        .route("/api/review/stats", get(review::review_stats))
        // Statistics
        .route("/api/stats/usage", get(stats::usage_stats))
        .route("/api/stats/review", get(stats::review_stats))
}
