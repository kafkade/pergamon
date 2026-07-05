// SPDX-License-Identifier: AGPL-3.0-only

//! Binary entry point for the pergamon sync server.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use pergamon_sync_server::{AppState, SyncStore, build_router};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// CLI arguments for the pergamon sync server.
#[derive(Debug, Parser)]
#[command(
    name = "pergamon-sync-server",
    version,
    about = "End-to-end-encrypted multi-device sync server for pergamon (AGPL-3.0)"
)]
struct Args {
    /// Host address to bind to.
    #[arg(long, default_value = "127.0.0.1", env = "PERGAMON_SYNC_HOST")]
    host: String,

    /// Port number to listen on.
    #[arg(long, default_value_t = 8787, env = "PERGAMON_SYNC_PORT")]
    port: u16,

    /// Path to the `SQLite` database file storing encrypted envelopes and blobs.
    ///
    /// Defaults to `$PERGAMON_SYNC_DATA_DIR/pergamon-sync.db` or
    /// `./pergamon-sync.db`.
    #[arg(long, env = "PERGAMON_SYNC_DB")]
    db_path: Option<PathBuf>,
}

/// Default database location: `$PERGAMON_SYNC_DATA_DIR` or the current directory.
fn default_db_path() -> PathBuf {
    std::env::var_os("PERGAMON_SYNC_DATA_DIR")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("pergamon-sync.db")
}

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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let db_path = args.db_path.unwrap_or_else(default_db_path);
    tracing::info!(path = %db_path.display(), "opening sync store");

    let store = SyncStore::open(&db_path)
        .with_context(|| format!("failed to open sync store at {}", db_path.display()))?;
    let state = AppState::new(store);

    let app = build_router(state);

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid bind address: {}:{}", args.host, args.port))?;

    tracing::info!(%addr, "starting sync server; it stores ciphertext only");

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("sync server stopped");
    Ok(())
}
