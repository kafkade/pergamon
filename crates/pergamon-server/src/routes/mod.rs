// SPDX-License-Identifier: AGPL-3.0-only

//! Route assembly for the pergamon REST API.

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

use axum::Router;
use axum::routing::{delete, get, patch, post};

use crate::state::AppState;

/// Build the complete application router.
///
/// Mounts all API endpoints under `/api/`, the server-rendered HTML views at
/// the root, and the health check at `/health`. Static file serving is added
/// separately in `main.rs` (embedded assets, or a disk directory override).
pub fn api_router() -> Router<AppState> {
    Router::new()
        // Health
        .route("/health", get(health::health))
        // Server-rendered web UI
        .route("/", get(web::index))
        .route("/inbox", get(web::inbox))
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
