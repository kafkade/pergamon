# ADR-018: Docker Deployment and Server Persistence

**Status**: Accepted  
**Date**: 2026-06-03  
**Deciders**: kafkade

## Context

ADR-016 decided that the Phase 5 web interface is an Axum server (`pergamon-web`) that renders HTML with HTMX, compiles templates and assets into a single binary, and requires no frontend build pipeline. ADR-017 decided that the web app uses a single owner password with Argon2id hashing, server-side sessions in SQLite, and runs plain HTTP behind a reverse proxy for TLS.

The Phase 5 acceptance criterion requires: "A self-hosted user can run pergamon web with Docker in under 10 minutes from documented setup."

This ADR defines how Docker packaging, data persistence, configuration, and server lifecycle work to meet that goal.

Key constraints:

- **Single-container deployment.** SQLite is embedded in the binary via `rusqlite/bundled` (ADR-006). There is no separate database container.
- **Data must survive container restarts.** The SQLite database and any stored content must persist across `docker compose down && docker compose up` cycles.
- **Works with common reverse proxies.** nginx, Caddy, and Traefik are standard in self-hosted environments.
- **Image size target: < 100 MB compressed.** The binary is a statically-linked Rust program with embedded assets. The runtime image needs minimal system packages.

## Decision

### Docker image build: multi-stage

The Docker image uses a two-stage build:

1. **Builder stage** (`rust:<version>-bookworm`): Compiles the `pergamon-web` binary in release mode. The Rust version matches the workspace's `rust-toolchain.toml` pin.
2. **Runtime stage** (`debian:bookworm-slim`): Contains only the compiled binary and minimal runtime dependencies.

Runtime packages required:

- `ca-certificates` — for HTTPS requests to external feeds during sync.

The runtime image does **not** install `libsqlite3`. SQLite is compiled from source and statically linked into the binary via the `rusqlite/bundled` feature flag (ADR-006). This guarantees FTS5 support regardless of the host distribution's SQLite version.

Build cache optimization:

- A `cargo chef` or dependency-caching layer copies `Cargo.toml` and `Cargo.lock` first, builds dependencies, then copies source code. This avoids recompiling all dependencies on every source change.
- `.dockerignore` excludes `target/`, `.git/`, `docs/`, and test fixtures from the build context.

### Container user: non-root

The official image runs as an unprivileged `pergamon` user (UID 1000, GID 1000). The data directory is created and owned by this user during the image build.

Running as non-root reduces the blast radius if the web application is compromised. This is important because `pergamon-web` is a network-facing service that renders personal content.

For bind-mounted host directories, the host directory must be writable by UID 1000, or the container must be started with a matching `--user` flag.

### Data persistence: single volume at /data

All persistent state lives under a single `/data` directory:

```text
/data/
├── pergamon.db          SQLite database (content, sessions, config)
├── pergamon.db-wal      SQLite WAL file (present when using WAL mode)
├── pergamon.db-shm      SQLite shared-memory file (present when using WAL mode)
└── exports/             Backup export staging area
```

The Dockerfile does not include a `VOLUME /data` directive. Implicit anonymous volumes create surprising behavior in one-off runs and derived images. Volume mounts are documented in Docker Compose and `docker run` examples instead.

### SQLite WAL mode

The web server enables SQLite WAL (Write-Ahead Logging) mode on startup. WAL provides:

- Concurrent readers do not block the writer, and the writer does not block readers. This matters because the web server handles multiple simultaneous HTTP requests.
- Better write performance for the typical append-heavy workload (saving articles, logging reviews).

WAL mode creates two sidecar files (`pergamon.db-wal` and `pergamon.db-shm`) alongside the main database file. These are part of the live database state.

Backup implications:

- **Application-level backup** (`export backup`) uses the SQLite backup API and produces a self-contained ZIP file. This is the recommended backup method while the container is running.
- **Volume-level backup** (copying `/data`) requires stopping the container first to ensure WAL is checkpointed and sidecar files are consistent.
- **Copying only `pergamon.db` while the container is running is unsafe.** The WAL file may contain uncommitted data.

WAL mode requires a local filesystem. Network filesystem mounts (NFS, SMB/CIFS) are unsupported because they do not provide the locking guarantees SQLite requires.

A `busy_timeout` of 5000 ms is set alongside WAL to handle brief lock contention between concurrent requests.

### Configuration: environment variables

Environment variables are the primary configuration mechanism for Docker deployments. CLI flags are available as overrides for non-Docker deployments.

| Variable | Default | Description |
|---|---|---|
| `PERGAMON_BIND` | `0.0.0.0:3000` | Socket bind address |
| `PERGAMON_DATA_DIR` | `/data` | Data directory for database and exports |
| `PERGAMON_LOG_LEVEL` | `info` | Log level: error, warn, info, debug, trace |
| `PERGAMON_LOG_FORMAT` | `json` | Log format: `json` (machine-readable) or `pretty` (human-readable) |
| `PERGAMON_PASSWORD_HASH` | — | Pre-computed Argon2id hash for headless setup (ADR-017) |
| `PERGAMON_TRUSTED_PROXIES` | — | CIDR ranges for trusted reverse proxies (ADR-017) |
| `PERGAMON_COOKIE_SECURE` | `false` | Force `Secure` cookie flag (ADR-017) |
| `PERGAMON_DISABLE_SETUP` | `false` | Refuse to start if no password is configured (ADR-017) |

**Bind address reconciliation with ADR-017:** ADR-017 specifies that `pergamon-web` listens on `127.0.0.1:3000` by default. Inside a Docker container, binding to localhost makes the server unreachable from outside the container. The Docker image sets `PERGAMON_BIND=0.0.0.0:3000` as its default. Native (non-Docker) installations retain the `127.0.0.1:3000` default in the binary. The `PERGAMON_BIND` variable overrides both.

No configuration file is used for the server. Environment variables are the standard configuration interface for containerized applications and are sufficient for a single-container, single-user deployment.

### Port binding and networking

The default port is `3000`. Docker Compose maps `3000:3000` by default.

The server runs plain HTTP. TLS termination is handled by a reverse proxy as specified in ADR-017. The deployment documentation includes sample configurations for:

- **Caddy** — automatic HTTPS with Let's Encrypt, minimal configuration.
- **nginx** — manual certificate configuration, widely deployed.

Direct public internet exposure without TLS remains unsupported and undocumented per ADR-017.

### Health check endpoint

`GET /health` returns HTTP 200 with a JSON body:

```json
{
  "status": "ok",
  "version": "0.7.0"
}
```

This endpoint:

- Does not require authentication.
- Verifies that the HTTP server is accepting requests and the database is accessible.
- Does not expose sensitive configuration, user data, or internal state.

The `pergamon-web` binary includes a `health-check` subcommand that performs an HTTP GET to the health endpoint using standard library networking. This avoids installing `curl` or `wget` in the minimal runtime image.

```dockerfile
HEALTHCHECK --interval=30s --timeout=5s --retries=3 --start-period=10s \
  CMD ["pergamon-web", "health-check", "--url", "http://127.0.0.1:3000/health"]
```

### Logging: tracing with structured output

Logging uses the `tracing` ecosystem with `tracing-subscriber`.

- **JSON format** by default (`PERGAMON_LOG_FORMAT=json`). JSON logs are machine-parseable and integrate with log aggregation tools (Loki, ELK, CloudWatch).
- **Pretty format** available for development and interactive debugging (`PERGAMON_LOG_FORMAT=pretty`).
- **Request logging** via `tower-http::trace`, logging method, path, status code, and latency for each request.
- **Startup banner** logged at INFO level: version, bind address, data directory, setup status (configured or awaiting setup), and number of migrations applied.

Log output goes to stdout. Docker captures stdout as container logs, accessible via `docker logs`.

### Database migrations on startup

`pergamon-storage` runs embedded migrations automatically when `Database::open()` is called. The web server inherits this behavior.

- Migrations are applied in order on every startup.
- Already-applied migrations are skipped.
- Applied migrations are logged at INFO level.
- No manual migration step is required.
- If a migration fails, the server logs the error and exits with a non-zero status code. Docker's restart policy will retry, and the health check will report failure.

Breaking schema changes, if any arise in future releases, will be documented in release notes with upgrade instructions.

### Graceful shutdown

The server handles `SIGTERM` and `SIGINT` by initiating a graceful shutdown:

1. Stop accepting new connections.
2. Wait for in-flight requests to complete, with a bounded timeout of 30 seconds.
3. Checkpoint the SQLite WAL.
4. Exit with status code 0.

Docker sends `SIGTERM` on `docker stop` and waits 10 seconds (configurable via `stop_grace_period`) before sending `SIGKILL`. The 30-second in-flight timeout is capped by Docker's grace period.

### Backup and restore

Two backup strategies are supported:

**Application-level backup (recommended while running):**

The `pergamon` CLI binary is included in the Docker image alongside `pergamon-web`. This avoids duplicating backup/restore logic in the web binary.

```sh
# Create a backup
docker exec pergamon pergamon export backup /data/exports/backup.zip

# Restore from backup (stop the web server first)
docker compose stop
docker exec pergamon pergamon import backup /data/exports/backup.zip
docker compose start
```

**Volume-level backup (requires container stop):**

```sh
docker compose stop
cp -a /var/lib/docker/volumes/pergamon-data/_data /backup/pergamon-$(date +%F)
docker compose start
```

Volume-level backup copies the raw SQLite files. The container must be stopped to ensure WAL consistency.

### Resource requirements

| Resource | Typical | Notes |
|---|---|---|
| Memory | 50–100 MB | Rust binary + SQLite in-process |
| Disk (database) | < 1 GB | Typical personal library |
| Disk (image) | < 100 MB compressed | Target for the runtime image |
| CPU | Minimal | Single-user workload, no background processing beyond feed sync |

### Upgrade path

1. Pull the new image (`docker compose pull`).
2. Restart the container (`docker compose up -d`).
3. Migrations run automatically on startup.
4. Data volume persists across container replacements.

Versioned image tags (e.g., `0.7.0`, `0.7`, `0`) are the recommended production practice. The `latest` tag tracks the most recent release but is not recommended for production deployments that require reproducibility.

### Docker Compose example

```yaml
services:
  pergamon:
    image: ghcr.io/kafkade/pergamon-web:0.7
    ports:
      - "3000:3000"
    volumes:
      - pergamon-data:/data
    environment:
      PERGAMON_PASSWORD_HASH: ${PERGAMON_PASSWORD_HASH}
      PERGAMON_LOG_LEVEL: info
    restart: unless-stopped

volumes:
  pergamon-data:
```

The password hash is referenced via `${PERGAMON_PASSWORD_HASH}` from a `.env` file or shell environment. The `.env` file should not be committed to version control, as it contains the password hash.

For interactive setup (no pre-configured password), omit `PERGAMON_PASSWORD_HASH`. The server will start in setup mode and print a setup token to the logs (see ADR-017).

### Docker Compose with Caddy (TLS)

```yaml
services:
  pergamon:
    image: ghcr.io/kafkade/pergamon-web:0.7
    volumes:
      - pergamon-data:/data
    environment:
      PERGAMON_PASSWORD_HASH: ${PERGAMON_PASSWORD_HASH}
      PERGAMON_TRUSTED_PROXIES: "172.16.0.0/12"
      PERGAMON_COOKIE_SECURE: "true"
    restart: unless-stopped

  caddy:
    image: caddy:2
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy-data:/data
    restart: unless-stopped

volumes:
  pergamon-data:
  caddy-data:
```

With `Caddyfile`:

```text
pergamon.example.com {
    reverse_proxy pergamon:3000
}
```

### .dockerignore

```text
target/
.git/
docs/
tests/fixtures/
*.md
LICENSE
```

### Image registry

Official images, if published, will use GitHub Container Registry under the repository owner's namespace: `ghcr.io/kafkade/pergamon-web`. Image signing and release automation are implementation details decided during CI setup, not in this ADR.

## Consequences

### Positive

- Single-container deployment with no external database dependency. `docker compose up` is the complete setup.
- Embedded SQLite with bundled compilation guarantees consistent behavior across all deployment environments.
- Non-root container reduces security risk for a network-facing personal data service.
- Built-in health check avoids runtime tool dependencies in the minimal image.
- WAL mode enables concurrent web access without blocking.
- Environment variable configuration follows twelve-factor app conventions and integrates naturally with Docker Compose, Kubernetes, and orchestration tools.
- Including the CLI binary in the image provides backup/restore without duplicating logic.
- Structured JSON logging integrates with standard log aggregation pipelines.
- Automatic migrations eliminate manual upgrade steps.

### Negative

- SQLite WAL sidecar files require users to understand that the database is more than one file. Volume-level backup documentation must address this.
- Non-root container requires host directory permissions to match the container UID for bind mounts.
- No built-in TLS means production deployments require a reverse proxy, adding a setup step.
- Single-container means no horizontal scaling. This is acceptable for a single-user personal tool.
- Including the CLI binary in the web image slightly increases image size but avoids maintaining a separate backup mechanism.

## Rejected Alternatives

- **Alpine Linux runtime image:** Rejected because musl libc can cause subtle issues with native Rust crates and SQLite. Debian bookworm-slim provides better compatibility with the Rust ecosystem at a modest size increase. Alpine remains a future optimization if image size becomes a concern.

- **Separate database container (PostgreSQL/MariaDB):** Rejected because it contradicts the local-first, single-binary design. SQLite is sufficient for a single-user workload and eliminates an entire infrastructure dependency.

- **Config file for server:** Rejected because environment variables are the standard configuration interface for containerized applications. A config file adds a volume mount, a file format to parse, and a precedence model to document, all for a single-user app with fewer than ten configuration options.

- **`VOLUME /data` Dockerfile directive:** Rejected because implicit anonymous volumes create surprising behavior in one-off `docker run` invocations and derived images. Explicit volume mounts in Compose and documentation are more predictable.

- **Built-in TLS in the application:** Rejected because TLS certificate management (renewal, ACME, storage) is complex and already well-solved by reverse proxies. Adding it to the application increases maintenance burden without meaningful benefit for the typical self-hosted deployment.

- **Embedding backup/restore in the web binary:** Rejected in favor of including the existing CLI binary. The CLI already implements `export backup` and `import backup` with full test coverage. Duplicating this in `pergamon-web` would create maintenance burden and divergence risk.
