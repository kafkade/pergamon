// SPDX-License-Identifier: AGPL-3.0-only

//! Route assembly for the pergamon REST API.

pub mod collections;
pub mod feeds;
pub mod health;
pub mod items;
pub mod tags;

use axum::Router;
use axum::routing::{delete, get, post};

use crate::state::AppState;

/// Build the complete application router.
///
/// Mounts all API endpoints under `/api/` and the health check at `/health`.
/// Static file serving is added separately in `main.rs` when configured.
pub fn api_router() -> Router<AppState> {
    Router::new()
        // Health
        .route("/health", get(health::health))
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
}
