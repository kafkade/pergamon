# ADR-026: Sync Server Deployment

**Status**: Accepted (amended 2026-07-27 for WAL — see [Amendment: WAL and
sidecar files](#amendment-wal-and-sidecar-files))
**Date**: 2026-07-09  
**Deciders**: kafkade

## Context

Phase 7 (epic #35) introduced optional, end-to-end-encrypted multi-device sync.
The client stack (`pergamon-crypto`, `pergamon-sync`) is Apache-2.0; the server
that shuttles ciphertext between devices, `pergamon-sync-server`, is a separate
AGPL-3.0 crate (ADR-008). It is a **blind relay**: it stores encrypted event
envelopes, content-addressed opaque blobs, and opaque onboarding artifacts, and
understands the structure of none of them (ADR-022, ADR-024). It never sees
plaintext.

Issue #130 requires packaging this server for self-hosting and managed hosting:
a Docker image, a Compose file, and operational documentation, such that a
self-hoster can stand it up from documented setup. This mirrors the Phase 5 web
Docker work (ADR-018) but for a different binary with a different security model.

Key differences from the web server (ADR-018):

- **Different binary and license.** `pergamon-sync-server` is AGPL-3.0, not the
  Apache-2.0 web server. The image ships an AGPL binary.
- **Different configuration surface.** Host/port/DB come from
  `PERGAMON_SYNC_HOST` / `PERGAMON_SYNC_PORT` / `PERGAMON_SYNC_DB` /
  `PERGAMON_SYNC_DATA_DIR`; the default port is **8787**.
- **Blind relay, no built-in auth.** The server has no application-level
  authentication. Confidentiality is guaranteed by client-side encryption;
  access control is an operational concern (reverse proxy or private network).
- **Ciphertext-only store, no bundled CLI.** There is no meaningful
  application-level backup of opaque ciphertext, and clients remain the source
  of truth, so backup is volume-level only and the image omits the CLI.

## Decision

### Docker image build: multi-stage

The image uses the same two-stage pattern as the web image (ADR-018):

1. **Builder stage** (`rust:1.96-bookworm`, matching the web image pin):
   compiles `pergamon-sync-server` in release mode, with BuildKit cache mounts
   for the cargo registry and target directory.
2. **Runtime stage** (`debian:bookworm-slim`): contains only the compiled
   binary, the AGPL license text, and `ca-certificates`.

SQLite is statically linked via `rusqlite/bundled` (ADR-006), so the runtime
image installs no `libsqlite3`. `.dockerignore` excludes `target/`, `.git/`,
`docs/`, fixtures, and the Docker files themselves from the build context.

The Docker files live at the repository root alongside the web ones —
`Dockerfile.sync-server` and `docker-compose.sync-server.yml` — so both services
share the single build context (the Cargo workspace) and the same
`.dockerignore`.

### Container user: non-root

The image runs as an unprivileged `pergamon` user (UID 1000, GID 1000). The
`/data` directory is created and owned by this user during the build. For
bind-mounted host directories, the directory must be writable by UID 1000.

### Data persistence: single volume at /data

All persistent state is the single SQLite database at `/data/pergamon-sync.db`.

> **Amended (WP-3e, #201).** This section originally read: "The server holds one
> `Mutex`-guarded connection with a `busy_timeout`, so under normal operation
> there are no WAL sidecar files." That is **no longer true.** The server now
> runs the database in **WAL mode** behind one writer connection and a bounded
> pool of reader connections (ADR-031), so `/data` normally contains
> `pergamon-sync.db`, `pergamon-sync.db-wal` and `pergamon-sync.db-shm`. All
> three belong to the database. See [Amendment: WAL and sidecar
> files](#amendment-wal-and-sidecar-files).

The database contains only ciphertext and blinded identifiers; it is useless
without the account key held by the client devices. The Dockerfile does not
include a `VOLUME /data` directive — volume mounts are documented in Compose and
`docker run` examples instead, matching ADR-018.

### Configuration: environment variables

Environment variables are the primary configuration mechanism; CLI flags are
available as overrides for non-Docker use.

| Variable | Flag | Default (image) | Description |
|---|---|---|---|
| `PERGAMON_SYNC_HOST` | `--host` | `0.0.0.0` | Bind address. Native default is `127.0.0.1`; the image binds all interfaces so the mapped port works. |
| `PERGAMON_SYNC_PORT` | `--port` | `8787` | Port to listen on. |
| `PERGAMON_SYNC_DATA_DIR` | — | `/data` | Directory for the database. |
| `PERGAMON_SYNC_DB` | `--db-path` | `/data/pergamon-sync.db` | Explicit database file path; overrides the data dir for the DB location. |
| `RUST_LOG` | — | `info` | Log filter. |

There is no config file and no auth-related configuration. This is sufficient
for a single-account, single-container deployment; access control lives at the
reverse proxy or network layer.

### Networking and reverse proxy

The default port is `8787`. The server runs plain HTTP with no built-in TLS and
**no built-in authentication**. For any non-local access it must sit behind a
reverse proxy that terminates TLS *and* enforces authentication, or on a trusted
private network. The deployment docs provide Caddy (automatic HTTPS) and nginx
(manual certificate) examples, both adding HTTP Basic auth.

This is the central security decision. End-to-end encryption protects the
*confidentiality* of user content — the operator and any third party who reaches
the port cannot read it — but it does not protect *availability*. Without an
auth layer an unauthenticated party could push junk blobs, consume storage, or
pull encrypted envelopes. A single shared credential at the proxy is adequate
for a personal deployment.

### Health check endpoint and subcommand

`GET /health` returns HTTP 200 with `{ "status": "ok", "version": "..." }`, or
503 if the store's writer lock has been poisoned by a panic. It requires no
authentication and exposes no user data.

Since WP-3e (#201) it deliberately does **not** probe the reader pool: a
saturated pool is transient load, not a fault, and failing the container health
check under load would make the orchestrator restart a perfectly healthy server.

The binary includes a `health-check` subcommand that performs an HTTP GET to the
endpoint, backing the container `HEALTHCHECK` so the minimal runtime image needs
no `curl`/`wget` — matching the web binary exactly:

```dockerfile
HEALTHCHECK --interval=30s --timeout=5s --retries=3 --start-period=10s \
  CMD ["pergamon-sync-server", "health-check", "--url", "http://127.0.0.1:8787/health"]
```

### Graceful shutdown

The server handles `SIGTERM` and `SIGINT` by stopping new connections and
draining in-flight requests before exiting cleanly. Docker sends `SIGTERM` on
`docker stop`.

### Backup and restore

Clients keep a full local copy and remain the source of truth, so a lost server
database is recoverable by re-syncing from a device. Backup is therefore
**volume-level only** and requires stopping the container so the files are
consistent (copy all of `/data`). The image intentionally omits the `pergamon`
CLI: the store is opaque ciphertext, so an application-level `export backup`
would add no value here (unlike the web image, where it exports readable
content).

> **Amended (WP-3e, #201).** Since the database runs in WAL mode, a backup must
> capture `pergamon-sync.db` **and** its `-wal` sidecar. The documented procedure
> — stop the container, archive the whole `/data` directory, start it again —
> stays correct, because a clean shutdown closes the last connection, which
> checkpoints the WAL and removes the sidecars; and the documented restore's
> `rm -f /data/*` correctly clears any stale ones. What is **no longer safe** is
> a *hot* copy of `pergamon-sync.db` alone: it omits recently committed data and
> may be unusable. Either stop the container, or snapshot all three files
> atomically.

Note that the web image's `export backup` produces a **plaintext ZIP of JSON
that excludes all key material** (account root key, device keys), so it must be
stored securely and cannot on its own recover an encrypted/sync-enabled account.
That image offers `export backup --encrypt` for an at-rest-encrypted archive and
`device-key export-package` to wrap the account root key for full recovery; this
blind-relay image, holding only ciphertext, needs neither.

### AGPL image obligations

The image ships an AGPL-3.0 binary. To make this explicit and compliant:

- The image carries `org.opencontainers.image.licenses=AGPL-3.0-only`.
- The AGPL license text is copied into the image at
  `/usr/share/doc/pergamon-sync-server/LICENSE`.
- The self-host guide documents the AGPL network-use clause (section 13):
  operators who run a **modified** server accessible over a network must offer
  their users the corresponding source. Running the unmodified upstream image
  carries no obligation beyond the AGPL's normal terms.

### CI

`.github/workflows/docker.yml` builds both images from a build matrix: on pull
requests it builds, loads, smoke-tests `/health`, and enforces the <100 MB
compressed size budget for each; on version tags it publishes
`ghcr.io/kafkade/pergamon-web` and `ghcr.io/kafkade/pergamon-sync-server` with
semver tags. This workflow is **not** part of the Terraform-managed required
status checks (`kafkade/github-infra`) — Docker builds are slow and do not gate
every PR — so adding the sync-server image requires no branch-protection change.

## Consequences

### Positive

- A self-hoster can stand up the sync server with a single
  `docker compose -f docker-compose.sync-server.yml up -d`.
- The AGPL boundary stays clean: only the AGPL binary ships in this image, built
  from the same workspace without linking server code into Apache crates.
- Reusing the web image's proven patterns (multi-stage build, non-root user,
  `/data` volume, binary self-probe, env-var config) keeps the two deployments
  consistent and the CI DRY.
- The blind-relay model plus explicit "add auth at the proxy" guidance gives a
  clear, honest security story.

### Negative

- The lack of built-in auth means a misconfigured deployment (exposed port, no
  proxy) is abusable for storage/DoS even though content stays encrypted. The
  docs mitigate this with prominent warnings, but it remains a footgun.
- Two root-level Docker file pairs (`Dockerfile` / `Dockerfile.sync-server` and
  their Compose files) add surface area to keep in sync.
- Including `reqwest` in the server solely for the health-check subcommand adds
  binary size, but keeps exact parity with the web image and stays within the
  compressed size budget.

## Rejected Alternatives

- **Built-in authentication in the server.** Rejected for this iteration: a
  reverse proxy already solves TLS and auth well, the deployment is
  single-account, and adding an auth/token model to a blind relay expands its
  scope and attack surface. It can be revisited if managed multi-tenant hosting
  demands it.

- **A `deploy/sync-server/` directory instead of root-level files.** Rejected
  for consistency: the web image uses root-level `Dockerfile` +
  `docker-compose.yml`, and the build context must be the workspace root
  regardless, so parallel root-level files are the least surprising.

- **A lightweight std-only TCP health probe (no `reqwest`).** Rejected in favor
  of exact parity with the web binary's `health-check` subcommand, so both
  images behave identically and the pattern is uniform.

- **Bundling the `pergamon` CLI for backups.** Rejected because the sync store is
  opaque ciphertext with no readable content to export, clients are the source
  of truth, and omitting the CLI keeps the image smaller.

- **Alpine runtime image.** Rejected for the same reason as ADR-018: musl libc
  can cause subtle issues with native Rust crates; `debian:bookworm-slim` is
  more compatible at a modest size cost.

## Amendment: WAL and sidecar files

**Date**: 2026-07-27 — **Driver**: WP-3e (#201), see ADR-031.

This ADR was written when the server held one `Mutex`-guarded SQLite connection
with a `busy_timeout` and the database's default rollback journal. WP-3e replaced
that with **WAL mode** behind one writer connection and a bounded reader pool, so
that concurrent tenants no longer serialize behind a single lock. Two statements
in this ADR were invalidated and have been corrected inline above:

1. **"Under normal operation there are no WAL sidecar files"** — no longer true.
   `/data` now normally holds `pergamon-sync.db`, `pergamon-sync.db-wal` and
   `pergamon-sync.db-shm`. All three are part of the database.
2. **The backup story.** Still volume-level only, and the documented
   stop → archive `/data` → start procedure is still correct: a clean shutdown
   checkpoints the WAL and removes the sidecars, and the documented restore's
   `rm -f /data/*` clears stale ones. The new hazard is a **hot** copy of
   `pergamon-sync.db` alone, which now silently omits recently committed data.
   Operators with a hand-rolled hot-copy script written against the old text must
   update it.

Nothing else in this ADR changes. The image, its configuration surface, the
blind-relay security model, the AGPL obligations, and the CI story are all
unaffected. `synchronous = NORMAL` (the standard WAL pairing) is compatible with
this ADR's premise that clients are the source of truth and a lost server
database is recoverable by re-syncing from a device.
