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

    /// Optional subcommand. When present, it runs and the server does not start.
    #[command(subcommand)]
    command: Option<Command>,
}

/// Subcommands for the pergamon sync server binary.
#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Probe a running server's health endpoint and exit.
    ///
    /// Performs an HTTP GET against the given URL and exits with status 0 when
    /// the response is HTTP 200, or a non-zero status otherwise. Used as the
    /// container `HEALTHCHECK` so the runtime image needs no `curl`/`wget`.
    HealthCheck(HealthCheckArgs),
}

/// Arguments for the `health-check` subcommand.
#[derive(Debug, clap::Args)]
struct HealthCheckArgs {
    /// URL of the health endpoint to probe.
    #[arg(long, default_value = "http://127.0.0.1:8787/health")]
    url: String,
}

/// Probe a server health endpoint and return an error if it is not healthy.
///
/// Sends an HTTP GET to `url` with a short timeout and succeeds only when the
/// response status is 2xx. This backs the container `HEALTHCHECK`, avoiding the
/// need for `curl`/`wget` in the minimal runtime image.
async fn run_health_check(url: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .context("failed to build HTTP client")?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("health check request to {url} failed"))?;

    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        anyhow::bail!("health check for {url} returned HTTP {status}");
    }
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

    // Subcommands short-circuit before opening the store or binding a port.
    if let Some(Command::HealthCheck(hc)) = &args.command {
        return run_health_check(&hc.url).await;
    }

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]
    #![allow(clippy::panic)]

    use super::*;

    /// Build a router backed by an in-memory store for tests.
    fn test_app() -> axum::Router {
        let store = SyncStore::open_in_memory().expect("open in-memory store");
        build_router(AppState::new(store))
    }

    #[test]
    fn health_check_subcommand_parses() {
        let args = Args::parse_from([
            "pergamon-sync-server",
            "health-check",
            "--url",
            "http://x/health",
        ]);
        match args.command {
            Some(Command::HealthCheck(hc)) => assert_eq!(hc.url, "http://x/health"),
            other => unreachable!("expected health-check subcommand, got {other:?}"),
        }
    }

    #[test]
    fn no_subcommand_runs_server() {
        let args = Args::parse_from(["pergamon-sync-server"]);
        assert!(args.command.is_none());
    }

    #[tokio::test]
    async fn health_check_fails_for_unreachable_url() {
        // Port 1 is privileged and not listening: the request must fail.
        let result = run_health_check("http://127.0.0.1:1/health").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn health_check_succeeds_against_running_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = test_app();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let url = format!("http://{addr}/health");
        let result = run_health_check(&url).await;
        assert!(result.is_ok(), "health check failed: {result:?}");
    }
}
