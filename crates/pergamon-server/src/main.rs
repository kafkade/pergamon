// SPDX-License-Identifier: AGPL-3.0-only

//! # pergamon-server
//!
//! Axum-based web server for pergamon — unified personal information system.
//!
//! This crate is licensed under AGPL-3.0. See the `LICENSE` file in this
//! crate's directory. All other pergamon crates are Apache-2.0.

mod error;
mod pagination;
mod routes;
mod state;
mod util;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use clap::Parser;
use pergamon_storage::Database;
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

// ======================================================================
// CLI arguments
// ======================================================================

/// CLI arguments for the pergamon web server.
#[derive(Debug, Parser)]
#[command(name = "pergamon-server", version, about = "Web server for pergamon")]
struct Args {
    /// Host address to bind to.
    #[arg(long, default_value = "127.0.0.1", env = "PERGAMON_HOST")]
    host: String,

    /// Port number to listen on.
    #[arg(long, default_value_t = 3000, env = "PERGAMON_PORT")]
    port: u16,

    /// Path to the `SQLite` database file.
    ///
    /// Defaults to `$PERGAMON_DATA_DIR/pergamon.db` or `./pergamon.db`.
    #[arg(long, env = "PERGAMON_DB")]
    db_path: Option<PathBuf>,

    /// Directory to serve static assets from (mounted at `/static`).
    #[arg(long, env = "PERGAMON_STATIC_DIR")]
    static_dir: Option<PathBuf>,
}

// ======================================================================
// Router construction
// ======================================================================

/// Build the Axum router with all routes and middleware.
fn build_router(state: AppState, static_dir: Option<&PathBuf>) -> Router {
    let mut app = routes::api_router();

    // Static assets: serve from a disk directory when configured (override),
    // otherwise serve the assets embedded in the binary.
    if let Some(dir) = static_dir {
        app = app.nest_service("/static", ServeDir::new(dir));
    } else {
        app = app.route(
            "/static/{file}",
            axum::routing::get(routes::static_assets::serve),
        );
    }

    app.with_state(state)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
}

// ======================================================================
// Graceful shutdown
// ======================================================================

/// Wait for a shutdown signal (Ctrl+C or SIGTERM on Unix).
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("failed to listen for ctrl+c: {e}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!("failed to listen for SIGTERM: {e}");
                // Fall through — ctrl_c will still work.
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

// ======================================================================
// Default database path
// ======================================================================

/// Default database location.
///
/// Uses `$PERGAMON_DATA_DIR` if set, otherwise the current directory.
/// Matches the behaviour of `pergamon-cli`.
fn default_db_path() -> PathBuf {
    std::env::var_os("PERGAMON_DATA_DIR")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("pergamon.db")
}

// ======================================================================
// Entry point
// ======================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialise structured logging with RUST_LOG env filter.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    // Resolve database path.
    let db_path = args.db_path.unwrap_or_else(default_db_path);
    tracing::info!(path = %db_path.display(), "opening database");

    let db = Database::open(&db_path)
        .with_context(|| format!("failed to open database at {}", db_path.display()))?;

    // Build async HTTP client with safety limits.
    let http = reqwest::Client::builder()
        .user_agent(format!("pergamon-server/{}", pergamon_core::VERSION))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("failed to build HTTP client")?;

    let state = AppState {
        db: Arc::new(std::sync::Mutex::new(db)),
        http,
    };

    let app = build_router(state, args.static_dir.as_ref());

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid bind address: {}:{}", args.host, args.port))?;

    tracing::info!(%addr, "starting server");

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("server stopped");
    Ok(())
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::significant_drop_tightening)]

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use pergamon_core::status::DocumentStatus;
    use tower::ServiceExt;

    /// Create an `AppState` backed by an in-memory database.
    fn test_state() -> AppState {
        let db = Database::open_in_memory()
            .unwrap_or_else(|e| unreachable!("failed to open in-memory DB: {e}"));
        let http = reqwest::Client::new();
        AppState {
            db: Arc::new(std::sync::Mutex::new(db)),
            http,
        }
    }

    fn test_app() -> Router {
        build_router(test_state(), None)
    }

    // ── Health ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn health_returns_ok() {
        let app = test_app();
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["status"], "ok");
        assert!(health["version"].is_string());
    }

    #[tokio::test]
    async fn health_still_works_with_static_dir() {
        let dir = std::env::temp_dir();
        let app = build_router(test_state(), Some(&dir));

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_route_returns_not_found() {
        let app = test_app();
        let req = Request::get("/nonexistent").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Items ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_items_empty() {
        let app = test_app();
        let req = Request::get("/api/items").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn get_item_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let req = Request::get(format!("/api/items/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_item_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let req = Request::delete(format!("/api/items/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_item_invalid_url() {
        let app = test_app();
        let body = serde_json::json!({ "url": "not-a-url" });
        let req = Request::post("/api/items")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_item_ftp_rejected() {
        let app = test_app();
        let body = serde_json::json!({ "url": "ftp://example.com/file" });
        let req = Request::post("/api/items")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // ── Feeds ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_feeds_empty() {
        let app = test_app();
        let req = Request::get("/api/feeds").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let feeds: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(feeds.is_empty());
    }

    #[tokio::test]
    async fn delete_feed_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let req = Request::delete(format!("/api/feeds/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Tags ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_tags_empty() {
        let app = test_app();
        let req = Request::get("/api/tags").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tags: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn add_tag_to_missing_item() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let body = serde_json::json!({ "name": "rust" });
        let req = Request::post(format!("/api/items/{id}/tags"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Collections ────────────────────────────────────────────────

    #[tokio::test]
    async fn list_collections_empty() {
        let app = test_app();
        let req = Request::get("/api/collections")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let cols: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(cols.is_empty());
    }

    #[tokio::test]
    async fn create_collection_success() {
        let app = test_app();
        let body = serde_json::json!({ "name": "Reading List" });
        let req = Request::post("/api/collections")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let col: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(col["name"], "Reading List");
    }

    #[tokio::test]
    async fn create_collection_empty_name() {
        let app = test_app();
        let body = serde_json::json!({ "name": "" });
        let req = Request::post("/api/collections")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn collection_items_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let req = Request::get(format!("/api/collections/{id}/items"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── PATCH items ────────────────────────────────────────────────

    #[tokio::test]
    async fn patch_item_and_get_with_tags() {
        // Insert a content item directly in the DB, then PATCH it.
        let state = test_state();
        let now = time::OffsetDateTime::now_utc();
        let item_id = uuid::Uuid::new_v4();
        let item = pergamon_core::model::ContentItem {
            id: item_id,
            url: Some("https://example.com/article".to_owned()),
            title: "Test Article".to_owned(),
            author: None,
            content_type: pergamon_core::content_type::ContentType::Article,
            status: pergamon_core::status::DocumentStatus::Inbox,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: now,
            updated_at: now,
            read_at: None,
        };
        {
            let db = state.db.lock().unwrap();
            db.insert_content_item(&item).unwrap();
        }

        let app = build_router(state, None);

        // PATCH status to "later" and add tags.
        let body = serde_json::json!({
            "status": "later",
            "tags": ["rust", "webdev"]
        });
        let req = Request::patch(format!("/api/items/{item_id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(resp["status"], "later");
        assert_eq!(resp["tags"].as_array().unwrap().len(), 2);
    }

    // ── Pagination ─────────────────────────────────────────────────

    #[tokio::test]
    async fn list_items_pagination_headers() {
        let app = test_app();
        let req = Request::get("/api/items?page=1&per_page=10")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-total-count"));
        assert!(response.headers().contains_key("link"));
    }

    // ── Web UI (HTML) ──────────────────────────────────────────────

    /// Insert a content item directly into a state's database.
    fn seed_item(
        state: &AppState,
        title: &str,
        status: pergamon_core::status::DocumentStatus,
    ) -> uuid::Uuid {
        let now = time::OffsetDateTime::now_utc();
        let id = uuid::Uuid::new_v4();
        let item = pergamon_core::model::ContentItem {
            id,
            url: Some(format!("https://example.com/{id}")),
            title: title.to_owned(),
            author: None,
            content_type: pergamon_core::content_type::ContentType::Article,
            status,
            content_text: Some("First paragraph.\n\nSecond paragraph.".to_owned()),
            excerpt: Some("An excerpt.".to_owned()),
            published_at: None,
            created_at: now,
            updated_at: now,
            read_at: None,
        };
        let db = state.db.lock().unwrap();
        db.insert_content_item(&item).unwrap();
        id
    }

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn index_redirects_to_inbox() {
        let app = test_app();
        let req = Request::get("/").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/inbox");
    }

    #[tokio::test]
    async fn inbox_renders_full_page() {
        let state = test_state();
        seed_item(&state, "Hello Inbox", DocumentStatus::Inbox);
        let app = build_router(state, None);

        let req = Request::get("/inbox").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let html = body_string(response).await;
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Hello Inbox"));
        assert!(html.contains("app-sidebar"));
    }

    #[tokio::test]
    async fn inbox_htmx_returns_list_fragment() {
        let state = test_state();
        seed_item(&state, "Fragment Item", DocumentStatus::Inbox);
        let app = build_router(state, None);

        let req = Request::get("/inbox")
            .header("HX-Request", "true")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let html = body_string(response).await;
        assert!(!html.contains("<!DOCTYPE html>"));
        assert!(html.contains("id=\"item-list\""));
        assert!(html.contains("Fragment Item"));
    }

    #[tokio::test]
    async fn inbox_accepts_filters_and_sort() {
        let app = test_app();
        let req = Request::get("/inbox?status=later&type=article&sort=title&page=1&per_page=10")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reader_renders_item() {
        let state = test_state();
        let id = seed_item(&state, "Readable Article", DocumentStatus::Inbox);
        let app = build_router(state, None);

        let req = Request::get(format!("/items/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let html = body_string(response).await;
        assert!(html.contains("Readable Article"));
        assert!(html.contains("First paragraph."));
        assert!(html.contains("Second paragraph."));
    }

    #[tokio::test]
    async fn reader_missing_item_is_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let req = Request::get(format!("/items/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn status_htmx_returns_row_fragment() {
        let state = test_state();
        let id = seed_item(&state, "Triage Me", DocumentStatus::Inbox);
        let app = build_router(state, None);

        let req = Request::post(format!("/items/{id}/status"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("action=archive"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let html = body_string(response).await;
        assert!(html.contains("data-item-row"));
        assert!(html.contains("item-row read"));
    }

    #[tokio::test]
    async fn status_htmx_reader_returns_status_text() {
        let state = test_state();
        let id = seed_item(&state, "Reader Triage", DocumentStatus::Inbox);
        let app = build_router(state, None);

        let req = Request::post(format!("/items/{id}/status"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("action=later&view=status"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let text = body_string(response).await;
        assert_eq!(text.trim(), "later");
    }

    #[tokio::test]
    async fn status_without_htmx_redirects() {
        let state = test_state();
        let id = seed_item(&state, "NoJs Triage", DocumentStatus::Inbox);
        let app = build_router(state, None);

        let req = Request::post(format!("/items/{id}/status"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("action=archive"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/inbox");
    }

    #[tokio::test]
    async fn bulk_action_returns_list_fragment() {
        let state = test_state();
        let id1 = seed_item(&state, "Bulk One", DocumentStatus::Inbox);
        let id2 = seed_item(&state, "Bulk Two", DocumentStatus::Inbox);
        let app = build_router(state, None);

        let req = Request::post("/items/bulk")
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("action=later&ids={id1}&ids={id2}")))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let html = body_string(response).await;
        assert!(html.contains("id=\"item-list\""));
    }

    #[tokio::test]
    async fn add_and_remove_tag() {
        let state = test_state();
        let id = seed_item(&state, "Tagged", DocumentStatus::Inbox);
        let app = build_router(state.clone(), None);

        let req = Request::post(format!("/items/{id}/tags"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("name=rust"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("rust"));

        let app = build_router(state, None);
        let req = Request::post(format!("/items/{id}/tags/rust/delete"))
            .header("HX-Request", "true")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(!html.contains(">rust<"));
    }

    #[tokio::test]
    async fn highlights_page_renders_with_source_context() {
        let state = test_state();
        let source_id = insert_item(&state, "Highlight Source", Some("A key paragraph here."));
        {
            let db = state.db.lock().unwrap();
            let _ = db
                .create_highlight(
                    source_id,
                    "A key paragraph",
                    Some("remember this"),
                    Some("yellow"),
                )
                .unwrap();
        }
        let app = build_router(state, None);

        let req = Request::get("/highlights").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("Highlight Source"));
        assert!(html.contains("A key paragraph"));
        assert!(html.contains("Export JSON"));
    }

    #[tokio::test]
    async fn highlight_note_update_htmx_updates_row() {
        let state = test_state();
        let source_id = insert_item(&state, "Highlight Source", Some("A key paragraph here."));
        let highlight_id = {
            let db = state.db.lock().unwrap();
            db.create_highlight(source_id, "A key paragraph", None, Some("yellow"))
                .unwrap()
                .id
        };
        let app = build_router(state.clone(), None);

        let req = Request::post(format!("/highlights/{highlight_id}/note"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("note=updated+from+web"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("updated from web"));

        let meta = {
            let db = state.db.lock().unwrap();
            db.get_highlight_meta(highlight_id).unwrap()
        };
        assert_eq!(meta.note.as_deref(), Some("updated from web"));
    }

    #[tokio::test]
    async fn notes_page_filters_by_query() {
        let state = test_state();
        let item_id = insert_item(&state, "Noted Item", None);
        {
            let db = state.db.lock().unwrap();
            let now = time::OffsetDateTime::now_utc();
            db.insert_note(&pergamon_core::model::Note {
                id: uuid::Uuid::new_v4(),
                content_item_id: item_id,
                body: "alpha idea".to_owned(),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
            db.insert_note(&pergamon_core::model::Note {
                id: uuid::Uuid::new_v4(),
                content_item_id: item_id,
                body: "beta thought".to_owned(),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        }
        let app = build_router(state, None);

        let req = Request::get("/notes?q=alpha").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("alpha idea"));
        assert!(!html.contains("beta thought"));
    }

    #[tokio::test]
    async fn notes_create_update_delete_htmx_cycle() {
        let state = test_state();
        let item_id = insert_item(&state, "Noted Item", None);
        let app = build_router(state.clone(), None);

        let req = Request::post("/notes/create")
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "content_item_id={item_id}&body=first+web+note"
            )))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("first web note"));

        let note_id = {
            let db = state.db.lock().unwrap();
            db.list_notes_for_item(item_id).unwrap()[0].id
        };

        let req = Request::post(format!("/notes/{note_id}/update"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("body=edited+web+note"))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("edited web note"));

        let req = Request::post(format!("/notes/{note_id}/delete"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(!html.contains("edited web note"));
    }

    #[tokio::test]
    async fn review_page_accepts_keyboard_rating_flow() {
        let state = test_state();
        let card_id = enable_review(&state);
        let app = build_router(state.clone(), None);

        let req = Request::get("/review").body(Body::empty()).unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("1 · Again"));
        assert!(html.contains("3 · Good"));

        let req = Request::post(format!("/review/{card_id}"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("rating=3"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let updated = {
            let db = state.db.lock().unwrap();
            db.get_review_card(card_id).unwrap()
        };
        assert_eq!(updated.review_count, 1);
    }

    #[tokio::test]
    async fn review_stats_web_and_api_include_dashboard_data() {
        let state = test_state();
        let card_id = enable_review(&state);
        let app = build_router(state, None);

        let req = Request::post(format!("/api/review/{card_id}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"rating":"good"}"#))
            .unwrap();
        let _ = app.clone().oneshot(req).await.unwrap();

        let req = Request::get("/review/stats").body(Body::empty()).unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("Monthly activity"));
        assert!(html.contains("Maturity distribution"));

        let req = Request::get("/api/review/stats")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        let report = json_body(response).await;
        assert!(report["monthly_history"].is_array());
    }

    #[tokio::test]
    async fn static_asset_is_served() {
        let app = test_app();
        let req = Request::get("/static/app.css").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains("text/css")
        );
    }

    #[tokio::test]
    async fn unknown_static_asset_is_not_found() {
        let app = test_app();
        let req = Request::get("/static/nope.xyz")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Test data helpers ──────────────────────────────────────────

    /// Insert a content item directly into the database, returning its ID.
    fn insert_item(state: &AppState, title: &str, content_text: Option<&str>) -> uuid::Uuid {
        let now = time::OffsetDateTime::now_utc();
        let id = uuid::Uuid::new_v4();
        let item = pergamon_core::model::ContentItem {
            id,
            url: Some(format!("https://example.com/{id}")),
            title: title.to_owned(),
            author: None,
            content_type: pergamon_core::content_type::ContentType::Article,
            status: pergamon_core::status::DocumentStatus::Inbox,
            content_text: content_text.map(str::to_owned),
            excerpt: None,
            published_at: None,
            created_at: now,
            updated_at: now,
            read_at: None,
        };
        let db = state.db.lock().unwrap();
        db.insert_content_item(&item).unwrap();
        id
    }

    /// Read a response body as JSON.
    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    // ── Search ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_requires_query() {
        let app = test_app();
        let req = Request::get("/api/search?q=").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn search_returns_ranked_hits() {
        let state = test_state();
        insert_item(&state, "Rust Async", Some("the quick brown fox jumps"));
        let app = build_router(state, None);

        let req = Request::get("/api/search?q=quick")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let hits = json_body(response).await;
        let arr = hits.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["rank"].is_number());
        assert_eq!(arr[0]["item"]["title"], "Rust Async");
    }

    #[tokio::test]
    async fn search_invalid_date_is_bad_request() {
        let app = test_app();
        let req = Request::get("/api/search?q=foo&since=not-a-date")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // ── Saved searches ─────────────────────────────────────────────

    #[tokio::test]
    async fn saved_searches_empty() {
        let app = test_app();
        let req = Request::get("/api/saved-searches")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let arr = json_body(response).await;
        assert!(arr.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn saved_search_create_and_list() {
        let state = test_state();
        let app = build_router(state, None);

        let body = serde_json::json!({ "name": "Rust later", "filter_query": "status:later" });
        let req = Request::post("/api/saved-searches")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = json_body(response).await;
        assert_eq!(created["name"], "Rust later");
        assert_eq!(created["is_smart"], true);

        let req = Request::get("/api/saved-searches")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        let arr = json_body(response).await;
        assert_eq!(arr.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn saved_search_invalid_filter_rejected() {
        let app = test_app();
        let body = serde_json::json!({ "name": "Bad", "filter_query": "" });
        let req = Request::post("/api/saved-searches")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // ── Highlights ─────────────────────────────────────────────────

    #[tokio::test]
    async fn highlights_empty() {
        let app = test_app();
        let req = Request::get("/api/highlights").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let arr = json_body(response).await;
        assert!(arr.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn highlight_create_list_patch_delete() {
        let state = test_state();
        let source_id = insert_item(&state, "Source", Some("a memorable passage here"));
        let app = build_router(state, None);

        // Create.
        let body = serde_json::json!({ "quote_text": "memorable passage", "color": "yellow" });
        let req = Request::post(format!("/api/items/{source_id}/highlights"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = json_body(response).await;
        let highlight_id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["highlight"]["color"], "yellow");

        // List for the source item.
        let req = Request::get(format!("/api/items/{source_id}/highlights"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        let arr = json_body(response).await;
        assert_eq!(arr.as_array().unwrap().len(), 1);

        // Patch the note.
        let body = serde_json::json!({ "note": "my thoughts" });
        let req = Request::patch(format!("/api/highlights/{highlight_id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let patched = json_body(response).await;
        assert_eq!(patched["highlight"]["note"], "my thoughts");
        // Color preserved across a note-only patch.
        assert_eq!(patched["highlight"]["color"], "yellow");

        // Delete.
        let req = Request::delete(format!("/api/highlights/{highlight_id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn create_highlight_missing_item() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let body = serde_json::json!({ "quote_text": "x" });
        let req = Request::post(format!("/api/items/{id}/highlights"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn patch_highlight_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let body = serde_json::json!({ "note": "x" });
        let req = Request::patch(format!("/api/highlights/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Notes ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn note_create_list_patch_delete() {
        let state = test_state();
        let item_id = insert_item(&state, "Item", None);
        let app = build_router(state, None);

        // Create.
        let body = serde_json::json!({ "body": "first note" });
        let req = Request::post(format!("/api/items/{item_id}/notes"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = json_body(response).await;
        let note_id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["body"], "first note");

        // List.
        let req = Request::get(format!("/api/items/{item_id}/notes"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        let arr = json_body(response).await;
        assert_eq!(arr.as_array().unwrap().len(), 1);

        // Patch.
        let body = serde_json::json!({ "body": "edited note" });
        let req = Request::patch(format!("/api/notes/{note_id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let patched = json_body(response).await;
        assert_eq!(patched["body"], "edited note");

        // Delete.
        let req = Request::delete(format!("/api/notes/{note_id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn create_note_missing_item() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let body = serde_json::json!({ "body": "x" });
        let req = Request::post(format!("/api/items/{id}/notes"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_note_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let req = Request::delete(format!("/api/notes/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Review ─────────────────────────────────────────────────────

    /// Create a highlight with a review card, returning the card ID.
    fn enable_review(state: &AppState) -> uuid::Uuid {
        let source_id = insert_item(state, "Review Source", Some("the studied fact"));
        let db = state.db.lock().unwrap();
        let highlight = db
            .create_highlight(source_id, "the studied fact", None, None)
            .unwrap();
        let now = time::OffsetDateTime::now_utc();
        let card = pergamon_core::model::ReviewCard {
            id: uuid::Uuid::new_v4(),
            content_item_id: highlight.id,
            state: pergamon_core::fsrs::CardState::New,
            stability: None,
            difficulty: None,
            due_at: now,
            last_reviewed_at: None,
            review_count: 0,
            lapse_count: 0,
            scheduled_days: None,
            created_at: now,
            updated_at: now,
        };
        db.insert_review_card(&card).unwrap();
        drop(db);
        card.id
    }

    #[tokio::test]
    async fn review_queue_empty() {
        let app = test_app();
        let req = Request::get("/api/review/queue")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let arr = json_body(response).await;
        assert!(arr.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn review_queue_and_submit() {
        let state = test_state();
        let card_id = enable_review(&state);
        let app = build_router(state, None);

        // Queue contains the new (due) card.
        let req = Request::get("/api/review/queue")
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        let arr = json_body(response).await;
        assert_eq!(arr.as_array().unwrap().len(), 1);

        // Submit a "good" rating; FSRS schedules it into the future.
        let body = serde_json::json!({ "rating": "good" });
        let req = Request::post(format!("/api/review/{card_id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let updated = json_body(response).await;
        assert_eq!(updated["review_count"], 1);
        assert_eq!(updated["state"], "review");

        // Card is no longer due.
        let req = Request::get("/api/review/queue")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        let arr = json_body(response).await;
        assert!(arr.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn submit_review_unknown_card() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let body = serde_json::json!({ "rating": "good" });
        let req = Request::post(format!("/api/review/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn submit_review_invalid_rating() {
        let state = test_state();
        let card_id = enable_review(&state);
        let app = build_router(state, None);
        let body = serde_json::json!({ "rating": "amazing" });
        let req = Request::post(format!("/api/review/{card_id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        // Invalid enum value fails JSON deserialization → 422.
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn review_stats_ok() {
        let app = test_app();
        let req = Request::get("/api/review/stats")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let report = json_body(response).await;
        assert_eq!(report["stats"]["total_cards"], 0);
    }

    // ── Statistics ─────────────────────────────────────────────────

    #[tokio::test]
    async fn stats_usage_ok() {
        let state = test_state();
        insert_item(&state, "An Item", Some("body text"));
        let app = build_router(state, None);
        let req = Request::get("/api/stats/usage")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let report = json_body(response).await;
        assert_eq!(report["overview"]["total_items"], 1);
    }

    #[tokio::test]
    async fn stats_review_ok() {
        let app = test_app();
        let req = Request::get("/api/stats/review")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let report = json_body(response).await;
        assert!(report["stats"].is_object());
    }

    // ── Web: Search ────────────────────────────────────────────────

    #[tokio::test]
    async fn search_page_renders_prompt_without_query() {
        let app = test_app();
        let req = Request::get("/search").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Type a query to search"));
    }

    #[tokio::test]
    async fn search_page_highlights_snippet() {
        let state = test_state();
        insert_item(
            &state,
            "Rust Patterns",
            Some("the quick brown fox jumps high"),
        );
        let app = build_router(state, None);

        let req = Request::get("/search?q=quick")
            .header("HX-Request", "true")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        // HTMX fragment only (no full page).
        assert!(!html.contains("<!DOCTYPE html>"));
        assert!(html.contains("id=\"search-results\""));
        assert!(html.contains("Rust Patterns"));
        assert!(html.contains("<mark>quick</mark>"));
    }

    #[tokio::test]
    async fn save_search_creates_smart_collection() {
        let state = test_state();
        let app = build_router(state.clone(), None);

        let req = Request::post("/search/save")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("name=Rust+stuff&dsl=text:rust+type:article"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        let coll = db.get_collection_by_name("Rust stuff").unwrap().unwrap();
        assert!(coll.is_smart);
        assert_eq!(coll.filter_query.as_deref(), Some("text:rust type:article"));
    }

    #[tokio::test]
    async fn save_search_rejects_invalid_dsl() {
        let app = test_app();
        let req = Request::post("/search/save")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("name=Bad&dsl="))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/search");
    }

    // ── Web: Bookmarks ─────────────────────────────────────────────

    /// Insert a bookmark-type content item, returning its ID.
    fn insert_bookmark(state: &AppState, title: &str) -> uuid::Uuid {
        let now = time::OffsetDateTime::now_utc();
        let id = uuid::Uuid::new_v4();
        let item = pergamon_core::model::ContentItem {
            id,
            url: Some(format!("https://example.com/bookmark/{id}")),
            title: title.to_owned(),
            author: None,
            content_type: pergamon_core::content_type::ContentType::Bookmark,
            status: pergamon_core::status::DocumentStatus::Inbox,
            content_text: None,
            excerpt: None,
            published_at: None,
            created_at: now,
            updated_at: now,
            read_at: None,
        };
        let db = state.db.lock().unwrap();
        db.insert_content_item(&item).unwrap();
        id
    }

    #[tokio::test]
    async fn bookmarks_page_lists_only_bookmarks() {
        let state = test_state();
        insert_bookmark(&state, "Saved Link");
        insert_item(&state, "An Article", Some("body"));
        let app = build_router(state, None);

        let req = Request::get("/bookmarks").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("Saved Link"));
        assert!(!html.contains("An Article"));
        assert!(html.contains("bookmark-grid"));
    }

    #[tokio::test]
    async fn bookmarks_layout_toggle_switches_to_list() {
        let state = test_state();
        insert_bookmark(&state, "Saved Link");
        let app = build_router(state, None);

        let req = Request::get("/bookmarks?layout=list")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("bookmark-list"));
    }

    #[tokio::test]
    async fn bookmarks_quick_add_creates_bookmark() {
        let state = test_state();
        let app = build_router(state.clone(), None);

        let req = Request::post("/bookmarks")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(
                "url=https://rust-lang.org/&title=Rust&tags=lang,systems",
            ))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/bookmarks");

        let db = state.db.lock().unwrap();
        let filter = pergamon_storage::ContentItemFilter {
            content_type: Some(pergamon_core::content_type::ContentType::Bookmark),
            ..pergamon_storage::ContentItemFilter::default()
        };
        let items = db.list_content_items_filtered(&filter, None, None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Rust");
    }

    #[tokio::test]
    async fn bookmarks_quick_add_rejects_bad_scheme() {
        let state = test_state();
        let app = build_router(state.clone(), None);

        let req = Request::post("/bookmarks")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("url=ftp://example.com/file"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        let filter = pergamon_storage::ContentItemFilter {
            content_type: Some(pergamon_core::content_type::ContentType::Bookmark),
            ..pergamon_storage::ContentItemFilter::default()
        };
        assert!(
            db.list_content_items_filtered(&filter, None, None)
                .unwrap()
                .is_empty()
        );
    }

    // ── Web: Tags ──────────────────────────────────────────────────

    /// Tag an item with a fresh tag, returning the tag ID.
    fn tag_item(state: &AppState, item_id: uuid::Uuid, name: &str) -> uuid::Uuid {
        let db = state.db.lock().unwrap();
        let tag = db.get_or_create_tag(name).unwrap();
        db.tag_content_item(item_id, tag.id).unwrap();
        tag.id
    }

    #[tokio::test]
    async fn tags_page_shows_cloud_with_counts() {
        let state = test_state();
        let item = insert_item(&state, "Tagged", Some("body"));
        tag_item(&state, item, "rust");
        let app = build_router(state, None);

        let req = Request::get("/tags").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("tag-cloud"));
        assert!(html.contains("rust"));
    }

    #[tokio::test]
    async fn tags_rename_changes_tag_name() {
        let state = test_state();
        let item = insert_item(&state, "Tagged", Some("body"));
        tag_item(&state, item, "rust");
        let app = build_router(state.clone(), None);

        let req = Request::post("/tags/rust/rename")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("new_name=rustlang"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        assert!(db.get_tag_by_name("rust").unwrap().is_none());
        assert!(db.get_tag_by_name("rustlang").unwrap().is_some());
    }

    #[tokio::test]
    async fn tags_merge_combines_tags() {
        let state = test_state();
        let item = insert_item(&state, "Tagged", Some("body"));
        tag_item(&state, item, "rust");
        tag_item(&state, item, "systems");
        let app = build_router(state.clone(), None);

        let req = Request::post("/tags/rust/merge")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("target=systems"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        assert!(db.get_tag_by_name("rust").unwrap().is_none());
        let systems = db.get_tag_by_name("systems").unwrap().unwrap();
        let tags = db.tags_for_item(item).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].id, systems.id);
    }

    #[tokio::test]
    async fn tags_delete_removes_tag() {
        let state = test_state();
        let item = insert_item(&state, "Tagged", Some("body"));
        tag_item(&state, item, "rust");
        let app = build_router(state.clone(), None);

        let req = Request::post("/tags/rust/delete")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        assert!(db.get_tag_by_name("rust").unwrap().is_none());
    }

    // ── Web: Collections ───────────────────────────────────────────

    /// Insert a regular collection, returning its ID.
    fn insert_collection(state: &AppState, name: &str) -> uuid::Uuid {
        let now = time::OffsetDateTime::now_utc();
        let coll = pergamon_core::model::Collection {
            id: uuid::Uuid::new_v4(),
            name: name.to_owned(),
            parent_id: None,
            sort_order: 0,
            is_smart: false,
            filter_query: None,
            created_at: now,
            updated_at: now,
        };
        let db = state.db.lock().unwrap();
        db.insert_collection(&coll).unwrap();
        coll.id
    }

    #[tokio::test]
    async fn collections_page_lists_regular_and_smart() {
        let state = test_state();
        insert_collection(&state, "Reading List");
        let app = build_router(state, None);

        let req = Request::get("/collections").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("Reading List"));
        assert!(html.contains("Regular collections"));
        assert!(html.contains("Smart collections"));
    }

    #[tokio::test]
    async fn collection_create_regular_redirects_to_detail() {
        let state = test_state();
        let app = build_router(state.clone(), None);

        let req = Request::post("/collections/create")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("name=Favorites&kind=regular"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            response.headers()["location"]
                .to_str()
                .unwrap()
                .starts_with("/collections/")
        );

        let db = state.db.lock().unwrap();
        assert!(db.get_collection_by_name("Favorites").unwrap().is_some());
    }

    #[tokio::test]
    async fn collection_create_smart_requires_valid_filter() {
        let state = test_state();
        let app = build_router(state.clone(), None);

        // Empty filter on a smart collection → rejected, back to /collections.
        let req = Request::post("/collections/create")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("name=Bad+Smart&kind=smart&filter_query="))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/collections");

        let db = state.db.lock().unwrap();
        assert!(db.get_collection_by_name("Bad Smart").unwrap().is_none());
    }

    #[tokio::test]
    async fn collection_detail_shows_members() {
        let state = test_state();
        let coll = insert_collection(&state, "Reading List");
        let item = insert_item(&state, "Member Article", Some("body"));
        {
            let db = state.db.lock().unwrap();
            db.add_to_collection(item, coll, 0).unwrap();
        }
        let app = build_router(state, None);

        let req = Request::get(format!("/collections/{coll}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("Reading List"));
        assert!(html.contains("Member Article"));
        assert!(html.contains("data-reorder-list"));
    }

    #[tokio::test]
    async fn collection_smart_detail_evaluates_filter() {
        let state = test_state();
        insert_bookmark(&state, "A Bookmark");
        insert_item(&state, "An Article", Some("body"));
        let coll = {
            let now = time::OffsetDateTime::now_utc();
            let coll = pergamon_core::model::Collection {
                id: uuid::Uuid::new_v4(),
                name: "Bookmarks Smart".to_owned(),
                parent_id: None,
                sort_order: 0,
                is_smart: true,
                filter_query: Some("type:bookmark".to_owned()),
                created_at: now,
                updated_at: now,
            };
            let db = state.db.lock().unwrap();
            db.insert_collection(&coll).unwrap();
            coll.id
        };
        let app = build_router(state, None);

        let req = Request::get(format!("/collections/{coll}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("A Bookmark"));
        assert!(!html.contains("An Article"));
        // Smart collections are not reorderable.
        assert!(!html.contains("data-reorder-list"));
    }

    #[tokio::test]
    async fn collection_reorder_sets_new_order() {
        let state = test_state();
        let coll = insert_collection(&state, "Ordered");
        let a = insert_item(&state, "Item A", Some("a"));
        let b = insert_item(&state, "Item B", Some("b"));
        {
            let db = state.db.lock().unwrap();
            db.add_to_collection(a, coll, 0).unwrap();
            db.add_to_collection(b, coll, 1).unwrap();
        }
        let app = build_router(state.clone(), None);

        // Reverse the order: B then A.
        let req = Request::post(format!("/collections/{coll}/reorder"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("ids={b}&ids={a}")))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        let ordered: Vec<uuid::Uuid> = db
            .list_collection_items(coll)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ordered, vec![b, a]);
    }

    #[tokio::test]
    async fn collection_move_item_down_swaps_order() {
        let state = test_state();
        let coll = insert_collection(&state, "Ordered");
        let a = insert_item(&state, "Item A", Some("a"));
        let b = insert_item(&state, "Item B", Some("b"));
        {
            let db = state.db.lock().unwrap();
            db.add_to_collection(a, coll, 0).unwrap();
            db.add_to_collection(b, coll, 1).unwrap();
        }
        let app = build_router(state.clone(), None);

        let req = Request::post(format!("/collections/{coll}/items/{a}/move"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("dir=down"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        let ordered: Vec<uuid::Uuid> = db
            .list_collection_items(coll)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ordered, vec![b, a]);
    }

    #[tokio::test]
    async fn collection_remove_item_drops_membership() {
        let state = test_state();
        let coll = insert_collection(&state, "Reading List");
        let item = insert_item(&state, "Member", Some("body"));
        {
            let db = state.db.lock().unwrap();
            db.add_to_collection(item, coll, 0).unwrap();
        }
        let app = build_router(state.clone(), None);

        let req = Request::post(format!("/collections/{coll}/items/{item}/remove"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        assert!(db.list_collection_items(coll).unwrap().is_empty());
    }

    #[tokio::test]
    async fn collection_update_filter_changes_dsl() {
        let state = test_state();
        let coll = {
            let now = time::OffsetDateTime::now_utc();
            let coll = pergamon_core::model::Collection {
                id: uuid::Uuid::new_v4(),
                name: "Smart".to_owned(),
                parent_id: None,
                sort_order: 0,
                is_smart: true,
                filter_query: Some("type:article".to_owned()),
                created_at: now,
                updated_at: now,
            };
            let db = state.db.lock().unwrap();
            db.insert_collection(&coll).unwrap();
            coll.id
        };
        let app = build_router(state.clone(), None);

        let req = Request::post(format!("/collections/{coll}/filter"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("filter_query=type:bookmark"))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let db = state.db.lock().unwrap();
        let updated = db.get_collection(coll).unwrap();
        assert_eq!(updated.filter_query.as_deref(), Some("type:bookmark"));
    }

    #[tokio::test]
    async fn collection_delete_removes_collection() {
        let state = test_state();
        let coll = insert_collection(&state, "Doomed");
        let app = build_router(state.clone(), None);

        let req = Request::post(format!("/collections/{coll}/delete"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/collections");

        let db = state.db.lock().unwrap();
        assert!(db.get_collection_by_name("Doomed").unwrap().is_none());
    }

    #[tokio::test]
    async fn collection_detail_missing_is_not_found() {
        let app = test_app();
        let id = uuid::Uuid::new_v4();
        let req = Request::get(format!("/collections/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
