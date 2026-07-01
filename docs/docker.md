# Self-hosting pergamon with Docker

This guide takes you from zero to a running, TLS-protected pergamon web server
(`pergamon-server`) in a container with persistent storage. It covers the quick
start, configuration, reverse proxy setup, backups, upgrades, troubleshooting,
and security.

If you have basic Docker experience, the [quick start](#quick-start) should get
you online in a few minutes; the reverse proxy section adds automatic HTTPS.

See [ADR-018: Docker Deployment](adr/018-docker-deployment.md) for the design
rationale behind this packaging.

> **Read this first — authentication.** The pergamon web UI does **not** have a
> built-in login. Anyone who can reach the server can read and modify your
> library. Only the `/admin` diagnostics page can be password-protected (see
> [Admin diagnostics auth](#admin-diagnostics-auth)). **Do not expose pergamon
> directly to the internet.** Run it on a trusted local network, or put it behind
> a reverse proxy that enforces TLS *and* authentication — see
> [Reverse proxy (TLS)](#reverse-proxy-tls) and
> [Security considerations](#security-considerations).

## Prerequisites

- **Docker Engine** 20.10 or newer.
- **Docker Compose** v2 (the `docker compose` subcommand; bundled with recent
  Docker Desktop and the `docker-compose-plugin` package on Linux).
- A **local filesystem** for the data volume. SQLite runs in WAL mode, which
  requires real file locking; network filesystems (NFS, SMB/CIFS) are not
  supported.

Check your versions:

```sh
docker --version
docker compose version
```

## Quick start

The repository ships a `Dockerfile` and a `docker-compose.yml`. From the repo
root:

```sh
docker compose up -d --build
```

This builds the image, creates a named `pergamon-data` volume, and starts the
server. It is reachable at <http://localhost:3000>.

Confirm it is healthy:

```sh
curl http://localhost:3000/health
# {"status":"ok","version":"..."}
```

Follow the logs (database migrations run automatically on first start):

```sh
docker compose logs -f pergamon
```

Stop it — your data is preserved in the `pergamon-data` volume:

```sh
docker compose down
```

That's the whole loop: `up` to start, `logs` to watch, `down` to stop. Data
survives `down`/`up` because it lives in a named volume, not the container.

### Running without Compose

If you prefer plain `docker`:

```sh
docker build -t pergamon:latest .

docker run -d --name pergamon \
  -p 3000:3000 \
  -v pergamon-data:/data \
  pergamon:latest
```

### Using a published image

If a prebuilt image is available for your platform, skip the build and reference
it directly instead of `build: .`:

```yaml
services:
  pergamon:
    image: ghcr.io/kafkade/pergamon:latest
    # ...
```

Pin a specific version tag rather than `latest` for reproducible deployments.

## Configuration reference

The server is configured entirely through environment variables (each has an
equivalent CLI flag for non-Docker use). The image ships with container-friendly
defaults: it binds `0.0.0.0:3000` and stores data in `/data`.

| Variable | Flag | Default (native) | Default (image) | Description |
|---|---|---|---|---|
| `PERGAMON_HOST` | `--host` | `127.0.0.1` | `0.0.0.0` | Address to bind. The image binds all interfaces so the port is reachable from outside the container. |
| `PERGAMON_PORT` | `--port` | `3000` | `3000` | Port to listen on. |
| `PERGAMON_DATA_DIR` | — | current dir | `/data` | Directory for the database (and default location for exports). |
| `PERGAMON_DB` | `--db-path` | `$PERGAMON_DATA_DIR/pergamon.db` | `/data/pergamon.db` | Explicit database file path. Overrides `PERGAMON_DATA_DIR` for the DB location. |
| `PERGAMON_STATIC_DIR` | `--static-dir` | embedded assets | embedded assets | Serve static assets from a directory instead of the assets baked into the binary. Rarely needed. |
| `RUST_LOG` | — | `info` | `info` | Log filter. Accepts `error`, `warn`, `info`, `debug`, `trace`, or per-target filters (e.g. `pergamon_server=debug,info`). |
| `PERGAMON_ADMIN_USER` | `--admin-user` | — | — | Username for HTTP Basic auth on the `/admin` diagnostics routes. See [Admin diagnostics auth](#admin-diagnostics-auth). |
| `PERGAMON_ADMIN_PASSWORD` | `--admin-password` | — | — | Password for HTTP Basic auth on `/admin`. **Both** user and password must be set to enable protection. |

Notes on defaults:

- **`PERGAMON_HOST`**: native installs default to `127.0.0.1` (loopback only);
  the Docker image overrides this to `0.0.0.0` so the mapped port works. Leave it
  at the image default unless you have a specific reason to change it.
- **`PERGAMON_DATA_DIR` vs `PERGAMON_DB`**: set `PERGAMON_DATA_DIR` to move the
  whole data directory; set `PERGAMON_DB` only if you need the database file at a
  path outside that directory.
- **`RUST_LOG`**: bump to `debug` temporarily when troubleshooting; keep `info`
  in normal operation to avoid noisy logs.

Set variables in `docker-compose.yml` under `environment:`, or pass `-e` flags to
`docker run`:

```yaml
services:
  pergamon:
    image: pergamon:latest
    environment:
      RUST_LOG: info
      PERGAMON_ADMIN_USER: admin
      PERGAMON_ADMIN_PASSWORD: ${PERGAMON_ADMIN_PASSWORD}
```

Reference secrets from a `.env` file (kept out of version control) rather than
hard-coding them — see [Security considerations](#security-considerations).

## Reverse proxy (TLS)

The server speaks plain HTTP and has no built-in TLS. For any access beyond
`localhost`, terminate TLS at a reverse proxy. Because pergamon also has no
built-in login, the reverse proxy is where you add authentication for the main
UI (for example, HTTP Basic auth).

Do **not** publish the pergamon HTTP port to the public internet directly. In the
examples below, the pergamon service does not map a host port at all — only the
proxy is exposed.

### Caddy (automatic HTTPS)

[Caddy](https://caddyserver.com/) provisions and renews Let's Encrypt
certificates automatically. This is the simplest path to HTTPS.

`docker-compose.yml`:

```yaml
services:
  pergamon:
    image: pergamon:latest
    build: .
    volumes:
      - pergamon-data:/data
    restart: unless-stopped
    # No host port published — only Caddy is exposed.

  caddy:
    image: caddy:2
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy-data:/data
      - caddy-config:/config
    restart: unless-stopped

volumes:
  pergamon-data:
  caddy-data:
  caddy-config:
```

`Caddyfile` — replace `pergamon.example.com` with your domain (its DNS must point
at this host, and ports 80/443 must be reachable for certificate issuance):

```text
pergamon.example.com {
    # Require a login before proxying. Generate a hash with:
    #   docker run --rm caddy:2 caddy hash-password --plaintext 'your-password'
    basic_auth {
        youruser $2a$14$replace_with_the_generated_hash
    }

    reverse_proxy pergamon:3000
}
```

Caddy forwards `X-Forwarded-Proto` and related headers automatically. Bring it
up with `docker compose up -d`; the certificate is issued on first request.

> Omit the `basic_auth` block only if the instance is reachable exclusively from
> a trusted private network. On the public internet, keep authentication in
> place — pergamon has none of its own.

### nginx (manual certificate)

Use nginx when you already manage certificates (e.g. via `certbot`, an internal
CA, or a corporate proxy). This example assumes nginx runs on the host with
certificates already in place and pergamon reachable on `127.0.0.1:3000`.

```nginx
server {
    listen 80;
    server_name pergamon.example.com;
    # Redirect all HTTP to HTTPS.
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl;
    server_name pergamon.example.com;

    ssl_certificate     /etc/ssl/certs/pergamon.crt;
    ssl_certificate_key /etc/ssl/private/pergamon.key;

    # Require a login. Create the file with:
    #   htpasswd -c /etc/nginx/pergamon.htpasswd youruser
    auth_basic           "pergamon";
    auth_basic_user_file /etc/nginx/pergamon.htpasswd;

    location / {
        proxy_pass http://127.0.0.1:3000;

        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # Allow slow operations (feed sync, large imports) to finish.
        proxy_connect_timeout 60s;
        proxy_send_timeout    120s;
        proxy_read_timeout    120s;
    }
}
```

Certificate renewal is your responsibility with this setup. If you use certbot,
its systemd timer or cron job renews automatically; reload nginx afterward
(`nginx -s reload`) so it picks up the new certificate.

If nginx runs as its own container in the same Compose project, publish pergamon
on the internal Compose network only (no host `ports:` mapping) and set
`proxy_pass http://pergamon:3000;` using the service name.

## Data persistence

All persistent state lives under `/data` inside the container:

```text
/data/
├── pergamon.db          SQLite database (content, config)
├── pergamon.db-wal      SQLite write-ahead log (present in WAL mode)
├── pergamon.db-shm      SQLite shared-memory index (present in WAL mode)
└── exports/             Backup export staging area (when you create backups)
```

The `pergamon.db-wal` and `pergamon.db-shm` files are **part of the live
database**, not scratch files. They matter for backups — see below.

The Compose file mounts a named volume (`pergamon-data`) at `/data`, so data
survives `docker compose down && docker compose up`. The image deliberately does
**not** declare a `VOLUME` directive — you control where the data lives.

### Bind mounts and permissions

If you bind-mount a host directory instead of a named volume, that directory
must be writable by **UID 1000** — the unprivileged `pergamon` user the container
runs as:

```sh
mkdir -p ./data && sudo chown 1000:1000 ./data
docker run -d --name pergamon \
  -p 3000:3000 \
  -v "$PWD/data:/data" \
  pergamon:latest
```

If the UID does not match, the container fails to open or create the database
with a permission error — see [Troubleshooting](#troubleshooting).

## Backup and restore

pergamon includes the `pergamon` CLI binary in the image alongside the server, so
you can create a portable, self-contained backup archive.

### Application-level backup (recommended, while running)

`export backup` uses the SQLite backup API and produces a single ZIP that is safe
to take while the server is running:

```sh
docker exec pergamon pergamon export backup --output /data/exports/backup.zip
docker cp pergamon:/data/exports/backup.zip ./backup.zip
```

> Note the `--output` (`-o`) flag: `export backup` takes the destination as a
> flag, not a positional argument.

Because the CLI inside the container reads `PERGAMON_DATA_DIR=/data`, it operates
on the same database the server uses — no extra flags needed.

### Volume-level backup (requires stopping the container)

Copying `/data` directly is only safe when the container is stopped, so the WAL
is checkpointed and the sidecar files are consistent:

```sh
docker compose stop
docker run --rm \
  -v pergamon-data:/data:ro \
  -v "$PWD":/backup \
  debian:bookworm-slim \
  tar czf /backup/pergamon-data-$(date +%F).tar.gz -C /data .
docker compose start
```

> **Never copy only `pergamon.db` while the container is running.** In WAL mode
> the most recent writes may live only in `pergamon.db-wal`; copying the main
> file alone yields a stale or corrupt backup. Use `export backup` (safe while
> running) or stop the container and copy the whole `/data` directory.

### Restore

Restore replaces the current database, so stop the server first. `import backup`
takes the archive path as a **positional** argument:

```sh
# Make the archive available inside the container's data dir, then stop serving.
docker cp ./backup.zip pergamon:/data/exports/backup.zip
docker compose stop

# Restore into the persisted volume.
docker compose run --rm pergamon pergamon import backup /data/exports/backup.zip

docker compose start
```

Verify afterward with `curl http://localhost:3000/health` and a quick look at the
UI.

## Upgrading

1. **Back up first** (see above) so you can roll back if needed.
2. Pull or rebuild the image:

   ```sh
   docker compose pull        # for a published image
   # or, when building locally:
   docker compose build --pull
   ```

3. Recreate the container:

   ```sh
   docker compose up -d
   ```

Schema migrations run automatically on startup, and the data volume persists
across container replacements. Watch the logs to confirm a clean start:

```sh
docker compose logs -f pergamon
```

Before a major upgrade, check the project's release notes / `CHANGELOG.md` for
any breaking changes or manual steps. Pinning a specific version tag (rather than
`latest`) makes upgrades deliberate and reproducible.

### Rollback

If an upgrade misbehaves, roll back by restoring the backup you took in step 1
onto the previous image version:

```sh
docker compose stop
# Point the image tag back to the previous version in docker-compose.yml, then:
docker compose up -d --no-start
docker compose run --rm pergamon pergamon import backup /data/exports/backup.zip
docker compose start
```

## Troubleshooting

### Container won't start / keeps restarting

Check the logs — startup errors (bad config, migration failure, unwritable data
dir) are printed there:

```sh
docker compose logs pergamon
# or, without Compose:
docker logs pergamon
```

Raise verbosity temporarily with `RUST_LOG=debug` to see more detail.

### Permission denied on the data directory

Symptom: the log shows a failure to open or create `/data/pergamon.db`. This
almost always means a bind-mounted host directory is not writable by UID 1000:

```sh
sudo chown -R 1000:1000 ./data
```

Named volumes (the default) don't have this problem because Docker initializes
their ownership from the image.

### Health check failing / container marked `unhealthy`

`docker ps` shows the health status in the `STATUS` column. A failing check
usually means the server isn't serving `GET /health` with HTTP 200. The endpoint
returns 503 if the database lock is poisoned (a prior panic) — restart the
container, and if it persists, check the logs for the underlying error and
consider restoring from a backup.

Probe it manually from the host:

```sh
curl -i http://localhost:3000/health
```

The container's built-in probe (`pergamon-server health-check`) uses the same
endpoint; no `curl`/`wget` is installed in the image.

### Database is locked

Transient "database is locked" messages under brief concurrent load are normal
and self-resolve. Persistent locking usually means the data directory is on an
unsupported network filesystem — move it to a local disk (WAL mode requires local
file locking).

### `/admin` returns 401 or is unexpectedly open

If `/admin` prompts for credentials you didn't set, or is open when you expected
it locked, review `PERGAMON_ADMIN_USER` / `PERGAMON_ADMIN_PASSWORD` — **both**
must be non-empty to enable protection. The startup logs state whether the admin
routes are protected or open.

## Security considerations

- **The main web UI has no authentication.** Anyone who can reach the port has
  full access to your library. Never expose the pergamon port to the public
  internet. For any non-local access, place it behind a reverse proxy that
  enforces **both** TLS and authentication (the Caddy and nginx examples above
  show HTTP Basic auth).
- **Always use TLS for non-local access.** The server is plain HTTP; terminate
  TLS at the proxy. Redirect HTTP to HTTPS.
- <a id="admin-diagnostics-auth"></a>**Protect `/admin`.** The `/admin`
  diagnostics dashboard exposes feed health, import history, and system stats.
  Set both `PERGAMON_ADMIN_USER` and `PERGAMON_ADMIN_PASSWORD` to require HTTP
  Basic auth on that subtree. If only one is set, the routes stay open and the
  server logs a warning. Note this protects only `/admin`, not the rest of the
  app — the reverse proxy is still responsible for the main UI.
- **Keep secrets out of version control.** Reference passwords and hashes from a
  `.env` file (Compose reads `.env` automatically) and add `.env` to
  `.gitignore`. Never commit credentials in `docker-compose.yml`.
- **Non-root by default.** The container runs as an unprivileged user (UID/GID
  1000), limiting the blast radius if the app is compromised. Don't override this
  with `--user 0` unless you have a specific, understood reason.
- **Keep the image current.** Rebuild/pull periodically to pick up base-image
  security updates, and back up before upgrading.

---

For the underlying design decisions (multi-stage build, WAL mode, non-root user,
health check, migrations), see
[ADR-018: Docker Deployment](adr/018-docker-deployment.md).
