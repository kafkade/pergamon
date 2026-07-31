// SPDX-License-Identifier: AGPL-3.0-only

//! Binary entry point for the pergamon sync server.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use opaque_ke::ServerSetup;
use pergamon_sync_server::auth::store::AuthStore;
use pergamon_sync_server::auth::throttle::ThrottleConfig;
use pergamon_sync_server::auth::{AuthState, PergamonCipherSuite, ServerMode};
use pergamon_sync_server::{
    AbuseConfig, AppState, FairnessConfig, PoolConfig, QuotaConfig, SyncStore,
    apply_abuse_controls, build_router_hardened, build_router_multitenant_hardened,
};
use rand::rngs::OsRng;
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

    /// Deployment mode: `blind` (default) keeps the single-account blind relay
    /// (ADR-026) with no auth plane; `multitenant` additionally mounts the
    /// OPAQUE auth control plane (WP-3a, #189).
    ///
    /// The OPAQUE auth plane is NOT YET EXTERNALLY SECURITY-REVIEWED and must
    /// not be deployed to production until the review checklist is signed off.
    #[arg(long, default_value = "blind", env = "PERGAMON_SYNC_MODE")]
    mode: String,

    /// Path to the separate `SQLite` auth database (multi-tenant mode only).
    ///
    /// Holds OPAQUE verifiers, the identity→account map, and throttling
    /// counters — never co-mingled with the blind content store. Defaults to
    /// `$PERGAMON_SYNC_DATA_DIR/pergamon-auth.db` or `./pergamon-auth.db`.
    #[arg(long, env = "PERGAMON_AUTH_DB")]
    auth_db: Option<PathBuf>,

    /// Path to the OPRF server-setup secret file (multi-tenant mode only).
    ///
    /// This is the OPAQUE server secret (comparable to a TLS private key). It is
    /// stored **outside** the auth database (design §1.8) and generated on first
    /// run if absent. Defaults to `$PERGAMON_SYNC_DATA_DIR/pergamon-oprf.key` or
    /// `./pergamon-oprf.key`.
    #[arg(long, env = "PERGAMON_AUTH_SERVER_SETUP")]
    auth_server_setup: Option<PathBuf>,

    // --- Pre-auth abuse controls (WP-4, #195) ---------------------------------
    // Safe, generous defaults so a normal single-account self-host is unaffected;
    // tune down for hostile exposure. See `pergamon_sync_server::abuse`.
    /// Default per-IP sustained request rate (requests/second). `0` disables the
    /// default rate-limit tier.
    #[arg(long, default_value_t = 50, env = "PERGAMON_RATE_LIMIT_RPS")]
    rate_limit_rps: u32,

    /// Default per-IP burst size. `0` disables the default rate-limit tier.
    #[arg(long, default_value_t = 100, env = "PERGAMON_RATE_LIMIT_BURST")]
    rate_limit_burst: u32,

    /// Strict per-IP sustained request rate (requests/second) for sensitive routes
    /// (upload, event-push, and — in multi-tenant mode — register/login). `0`
    /// disables the strict tier.
    #[arg(long, default_value_t = 20, env = "PERGAMON_STRICT_RATE_LIMIT_RPS")]
    strict_rate_limit_rps: u32,

    /// Strict per-IP burst size for sensitive routes. `0` disables the strict tier.
    #[arg(long, default_value_t = 40, env = "PERGAMON_STRICT_RATE_LIMIT_BURST")]
    strict_rate_limit_burst: u32,

    /// Default maximum request body size in bytes (control/JSON routes).
    #[arg(long, default_value_t = 16 * 1024 * 1024, env = "PERGAMON_MAX_BODY_BYTES")]
    max_body_bytes: usize,

    /// Maximum blob-upload body size in bytes (and the global body backstop).
    #[arg(long, default_value_t = 64 * 1024 * 1024, env = "PERGAMON_UPLOAD_MAX_BYTES")]
    upload_max_bytes: usize,

    /// Maximum number of requests processed concurrently before excess is shed with
    /// `503` (storage-DoS isolation). `0` disables the concurrency limit.
    #[arg(long, default_value_t = 256, env = "PERGAMON_MAX_CONCURRENCY")]
    max_concurrency: usize,

    /// Trust reverse-proxy client-IP headers (`X-Forwarded-For` / `X-Real-Ip` /
    /// `Forwarded`) for rate limiting instead of the socket peer IP. Only enable
    /// behind a trusted proxy (ADR-026) — otherwise callers can spoof their IP.
    #[arg(long, default_value_t = false, env = "PERGAMON_TRUST_PROXY_HEADERS")]
    trust_proxy_headers: bool,

    // --- Per-tenant storage quotas (WP-3d, #198) ------------------------------
    // Opt-in: `0` means unlimited, so the default is byte-for-byte unchanged for
    // blind self-hosts and existing multi-tenant deployments. Measured on
    // ciphertext size + object counts only (content-blind). See
    // `pergamon_sync_server::quota`.
    /// Maximum total ciphertext bytes (blobs + event payloads) a single account
    /// may store before writes are refused with `507 QUOTA_EXCEEDED`. `0` =
    /// unlimited (the default).
    #[arg(long, default_value_t = 0, env = "PERGAMON_MAX_ACCOUNT_BYTES")]
    max_account_bytes: u64,

    /// Maximum total stored objects (blobs + events) a single account may hold
    /// before writes are refused with `507 QUOTA_EXCEEDED`. `0` = unlimited (the
    /// default).
    #[arg(long, default_value_t = 0, env = "PERGAMON_MAX_ACCOUNT_OBJECTS")]
    max_account_objects: u64,

    // --- Concurrency and per-tenant fairness (WP-3e, #201) --------------------
    // The content store runs one writer connection plus a bounded pool of reader
    // connections over a WAL database, so concurrent tenants no longer serialize
    // behind a single lock. Defaults keep a single-tenant self-host sane.
    /// Number of pooled `SQLite` reader connections.
    ///
    /// `SQLite` allows exactly one writer at a time even in WAL mode, so this
    /// sizes the **read** concurrency only; writes always serialize.
    #[arg(long, default_value_t = 8, env = "PERGAMON_READ_POOL_SIZE")]
    read_pool_size: usize,

    /// How long a request waits for a free reader connection (or a per-tenant
    /// slot) before being shed with `503`, in milliseconds.
    #[arg(
        long,
        default_value_t = 5000,
        env = "PERGAMON_STORE_CHECKOUT_TIMEOUT_MS"
    )]
    store_checkout_timeout_ms: u64,

    /// Maximum store operations a single account may have in flight, so one
    /// heavy tenant cannot hold every pooled connection.
    ///
    /// `0` (the default) means "derive from the pool": `read-pool-size - 1`, so
    /// a tenant can never take the last connection and another tenant always
    /// gets in. Set an explicit value to tighten it, or use
    /// `--no-tenant-concurrency-limit` to switch the cap off entirely.
    #[arg(long, default_value_t = 0, env = "PERGAMON_MAX_TENANT_CONCURRENCY")]
    max_tenant_concurrency: usize,

    /// Disable the per-tenant concurrency cap entirely (WP-3e, #201).
    ///
    /// Every tenant may then use the whole reader pool, which is fine for a
    /// single-account self-host and unwise for managed multi-tenant hosting.
    #[arg(
        long,
        default_value_t = false,
        env = "PERGAMON_NO_TENANT_CONCURRENCY_LIMIT"
    )]
    no_tenant_concurrency_limit: bool,

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

/// Default auth-database location (multi-tenant mode).
fn default_auth_db_path() -> PathBuf {
    std::env::var_os("PERGAMON_SYNC_DATA_DIR")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("pergamon-auth.db")
}

/// Default OPRF server-setup secret location (multi-tenant mode).
fn default_server_setup_path() -> PathBuf {
    std::env::var_os("PERGAMON_SYNC_DATA_DIR")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("pergamon-oprf.key")
}

/// Load the OPRF server secret from `path`, or generate and persist a new one on
/// first run.
///
/// The secret is stored **outside** the auth database (design §1.8). On Unix the
/// generated file is written with `0600` permissions.
fn load_or_create_server_setup(path: &Path) -> Result<ServerSetup<PergamonCipherSuite>> {
    if path.exists() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read OPRF server setup at {}", path.display()))?;
        ServerSetup::<PergamonCipherSuite>::deserialize(&bytes)
            .map_err(|e| anyhow::anyhow!("failed to parse OPRF server setup: {e}"))
    } else {
        let mut rng = OsRng;
        let setup = ServerSetup::<PergamonCipherSuite>::new(&mut rng);
        std::fs::write(path, setup.serialize())
            .with_context(|| format!("failed to write OPRF server setup to {}", path.display()))?;
        restrict_permissions(path);
        tracing::info!(path = %path.display(), "generated a new OPRF server-setup secret");
        Ok(setup)
    }
}

/// Best-effort tighten of a secret file's permissions to owner-only on Unix.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(path = %path.display(), error = %e, "failed to restrict secret file permissions");
    }
}

/// No-op on non-Unix platforms.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

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
// Startup wiring: parse args, open the store, pick a router, bind. Splitting it
// would scatter the configuration story across helpers for no benefit.
#[allow(clippy::too_many_lines)]
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

    // WP-3d (#198) per-tenant storage quota. `0` = unlimited (the default), so a
    // store built without an explicit cap is behavior-identical to before.
    let quota = QuotaConfig {
        max_account_bytes: args.max_account_bytes,
        max_account_objects: args.max_account_objects,
    };
    if !quota.is_unlimited() {
        tracing::info!(
            max_account_bytes = quota.max_account_bytes,
            max_account_objects = quota.max_account_objects,
            "per-tenant storage quota enabled (0 = unlimited)"
        );
    }

    // WP-3e (#201) concurrency: WAL + one writer connection + a bounded reader
    // pool, replacing the single process-wide store mutex.
    let checkout_timeout = std::time::Duration::from_millis(args.store_checkout_timeout_ms);
    let pool = PoolConfig {
        size: args.read_pool_size,
        checkout_timeout,
    };

    let store = SyncStore::open_with_pool(&db_path, pool)
        .with_context(|| format!("failed to open sync store at {}", db_path.display()))?
        .with_quota(quota);

    let fairness = if args.no_tenant_concurrency_limit {
        FairnessConfig::disabled()
    } else if args.max_tenant_concurrency > 0 {
        FairnessConfig {
            max_tenant_concurrency: args.max_tenant_concurrency,
            wait_timeout: checkout_timeout,
        }
    } else {
        FairnessConfig::for_pool(store.read_pool_size(), checkout_timeout)
    };
    tracing::info!(
        journal_mode = %store.journal_mode().unwrap_or_else(|_| "unknown".to_owned()),
        read_pool_size = store.read_pool_size(),
        checkout_timeout_ms = args.store_checkout_timeout_ms,
        max_tenant_concurrency = fairness.max_tenant_concurrency,
        "store concurrency configured (0 tenant concurrency = no per-tenant cap; \
         SQLite allows one writer at a time regardless of pool size)"
    );

    let state = AppState::with_fairness(store, fairness);

    let mode = ServerMode::from_env_value(&args.mode).unwrap_or_else(|| {
        tracing::warn!(mode = %args.mode, "unknown PERGAMON_SYNC_MODE; defaulting to blind");
        ServerMode::Blind
    });

    // WP-4 (#195) pre-auth abuse controls. Built from CLI/env with safe defaults.
    let abuse = AbuseConfig {
        rate_limit_rps: args.rate_limit_rps,
        rate_limit_burst: args.rate_limit_burst,
        strict_rate_limit_rps: args.strict_rate_limit_rps,
        strict_rate_limit_burst: args.strict_rate_limit_burst,
        max_body_bytes: args.max_body_bytes,
        upload_max_bytes: args.upload_max_bytes,
        max_concurrency: args.max_concurrency,
        trust_proxy_headers: args.trust_proxy_headers,
    };

    // WP-4/WP-3a conflict seam: this router-construction block is the point where
    // WP-4 abuse controls and the WP-3a auth plane both touch `main.rs`. Keep it
    // small; the abuse wiring is just "pick a hardened builder, then wrap globally".
    let app = match mode {
        ServerMode::Blind => {
            tracing::info!("mode=blind: single-account blind relay (ADR-026), no auth plane");
            build_router_hardened(state, &abuse)
        }
        ServerMode::Multitenant => {
            tracing::warn!(
                "mode=multitenant: mounting the OPAQUE auth control plane (WP-3a). \
                 This auth code is NOT YET EXTERNALLY SECURITY-REVIEWED — do not deploy \
                 to production until the review checklist (design §1.11) is signed off."
            );
            let auth_db_path = args.auth_db.unwrap_or_else(default_auth_db_path);
            let setup_path = args
                .auth_server_setup
                .unwrap_or_else(default_server_setup_path);
            tracing::info!(path = %auth_db_path.display(), "opening auth store");
            let auth_store = AuthStore::open(&auth_db_path).with_context(|| {
                format!("failed to open auth store at {}", auth_db_path.display())
            })?;
            let server_setup = load_or_create_server_setup(&setup_path)?;
            let auth_state =
                AuthState::new(auth_store, server_setup, "v1", ThrottleConfig::default());
            build_router_multitenant_hardened(state, auth_state, &abuse)
        }
    };

    // Global controls (default per-IP rate limit, concurrency/load-shed, absolute
    // body backstop) wrap the whole app — so they also cover the auth routes and
    // any future routes without per-route wiring.
    let app = apply_abuse_controls(app, &abuse);

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid bind address: {}:{}", args.host, args.port))?;

    tracing::info!(%addr, "starting sync server; it stores ciphertext only");

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    // `into_make_service_with_connect_info` exposes the socket peer IP as
    // `ConnectInfo<SocketAddr>`, which the per-IP rate limiter keys on.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
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
        pergamon_sync_server::build_router(AppState::new(store))
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
