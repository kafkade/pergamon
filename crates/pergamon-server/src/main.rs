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
    let mut app = routes::api_router().with_state(state);

    // Mount static file serving when a directory is configured.
    if let Some(dir) = static_dir {
        app = app.nest_service("/static", ServeDir::new(dir));
    }

    app.layer(CompressionLayer::new())
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

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
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
}
