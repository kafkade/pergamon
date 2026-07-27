# Self-hosting the pergamon sync server with Docker

This guide takes you from zero to a running pergamon **sync server**
(`pergamon-sync-server`) in a container with persistent storage. It covers the
quick start, configuration, putting the server behind a reverse proxy (TLS +
authentication), connecting clients, backups, upgrades, and the security model.

The sync server is **optional**. pergamon is local-first: your local database is
always the source of truth. The sync server exists only to shuttle
**end-to-end-encrypted** data between your own devices. See
[ADR-026: Sync Server Deployment](adr/026-sync-server-deployment.md) for the
design rationale, and [ADR-022](adr/022-sync-protocol-and-envelope-model.md) /
[ADR-024](adr/024-device-onboarding-and-key-lifecycle.md) for the protocol and
key model.

> **Read this first — the server is a blind relay with no built-in auth.**
> The sync server never sees your plaintext: it stores only ciphertext and
> opaque onboarding artifacts, and it cannot read your titles, notes, tags,
> URLs, or content even if it wanted to. **But it has no authentication of its
> own** — anyone who can reach the port can push and pull encrypted blobs for
> any account (they still can't decrypt them, but they can consume storage or
> disrupt sync). **Do not expose it directly to the internet.** Put it behind a
> reverse proxy that enforces TLS *and* authentication, or keep it on a trusted
> private network — see [Reverse proxy (TLS + auth)](#reverse-proxy-tls--auth)
> and [Security considerations](#security-considerations).

## Prerequisites

- **Docker Engine** 20.10 or newer.
- **Docker Compose** v2 (the `docker compose` subcommand; bundled with recent
  Docker Desktop and the `docker-compose-plugin` package on Linux).
- A **local filesystem** for the data volume. SQLite requires real file
  locking; network filesystems (NFS, SMB/CIFS) are not supported.

Check your versions:

```sh
docker --version
docker compose version
```

## Quick start

The repository ships a `Dockerfile.sync-server` and a
`docker-compose.sync-server.yml`. From the repo root:

```sh
docker compose -f docker-compose.sync-server.yml up -d --build
```

This builds the image, creates a named `pergamon-sync-data` volume, and starts
the server. It listens on <http://localhost:8787>.

Confirm it is healthy:

```sh
curl http://localhost:8787/health
# {"status":"ok","version":"..."}
```

Follow the logs:

```sh
docker compose -f docker-compose.sync-server.yml logs -f pergamon-sync
```

Stop it — your data is preserved in the `pergamon-sync-data` volume:

```sh
docker compose -f docker-compose.sync-server.yml down
```

### Running without Compose

If you prefer plain `docker`:

```sh
docker build -f Dockerfile.sync-server -t pergamon-sync-server:latest .

docker run -d --name pergamon-sync \
  -p 8787:8787 \
  -v pergamon-sync-data:/data \
  pergamon-sync-server:latest
```

### Using a published image

If a prebuilt image is available for your platform, skip the build and reference
it directly instead of `build:`:

```yaml
services:
  pergamon-sync:
    image: ghcr.io/kafkade/pergamon-sync-server:latest
    # ...
```

Pin a specific version tag rather than `latest` for reproducible deployments.

## Configuration reference

The server is configured entirely through environment variables (each has an
equivalent CLI flag for non-Docker use). The image ships with container-friendly
defaults: it binds `0.0.0.0:8787` and stores its database in `/data`.

| Variable | Flag | Default (native) | Default (image) | Description |
|---|---|---|---|---|
| `PERGAMON_SYNC_HOST` | `--host` | `127.0.0.1` | `0.0.0.0` | Address to bind. The image binds all interfaces so the port is reachable from outside the container. |
| `PERGAMON_SYNC_PORT` | `--port` | `8787` | `8787` | Port to listen on. |
| `PERGAMON_SYNC_DATA_DIR` | — | current dir | `/data` | Directory for the database. |
| `PERGAMON_SYNC_DB` | `--db-path` | `$PERGAMON_SYNC_DATA_DIR/pergamon-sync.db` | `/data/pergamon-sync.db` | Explicit database file path. Overrides `PERGAMON_SYNC_DATA_DIR` for the DB location. |
| `RUST_LOG` | — | `info` | `info` | Log filter. Accepts `error`, `warn`, `info`, `debug`, `trace`, or per-target filters (e.g. `pergamon_sync_server=debug,info`). |

Notes on defaults:

- **`PERGAMON_SYNC_HOST`**: native installs default to `127.0.0.1` (loopback
  only); the Docker image overrides this to `0.0.0.0` so the mapped port works.
  Leave it at the image default unless you have a specific reason to change it.
- **`RUST_LOG`**: bump to `debug` temporarily when troubleshooting; keep `info`
  in normal operation to avoid noisy logs.

The server has **no application-level authentication settings** — access control
is the reverse proxy's or network's responsibility (see below). Set variables in
`docker-compose.sync-server.yml` under `environment:`, or pass `-e` flags to
`docker run`.

## Reverse proxy (TLS + auth)

The server speaks plain HTTP, has no built-in TLS, and has no built-in
authentication. For any access beyond `localhost` you **must** front it with a
reverse proxy that terminates TLS and adds an authentication layer. Clients
connect over HTTPS in practice.

Do **not** publish the sync-server HTTP port to the public internet directly. In
the examples below the sync service does not map a host port at all — only the
proxy is exposed.

> **Why auth matters even though the data is encrypted.** End-to-end encryption
> protects the *confidentiality* of your content: the server (and anyone who
> reaches it) cannot read it. It does **not** protect *availability*. Without an
> auth layer, an unauthenticated party can push junk blobs, consume disk, or
> pull your (encrypted) envelopes. A single shared credential at the proxy is
> enough for a personal, single-account deployment.

### Caddy (automatic HTTPS)

[Caddy](https://caddyserver.com/) provisions and renews Let's Encrypt
certificates automatically.

`docker-compose.sync-server.yml` (proxy variant):

```yaml
services:
  pergamon-sync:
    image: ghcr.io/kafkade/pergamon-sync-server:latest
    volumes:
      - pergamon-sync-data:/data
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
  pergamon-sync-data:
  caddy-data:
  caddy-config:
```

`Caddyfile` — replace `sync.example.com` with your domain (its DNS must point at
this host, and ports 80/443 must be reachable for certificate issuance):

```text
sync.example.com {
    # Require a login before proxying. Generate a hash with:
    #   docker run --rm caddy:2 caddy hash-password --plaintext 'your-password'
    basic_auth {
        youruser $2a$14$replace_with_the_generated_hash
    }

    reverse_proxy pergamon-sync:8787
}
```

Bring it up with
`docker compose -f docker-compose.sync-server.yml up -d`; the certificate is
issued on first request. Clients then point at
`https://sync.example.com` and supply the reverse-proxy credential through the
environment (see [Connecting a client](#connecting-a-client) below).

### nginx (manual certificate)

Use nginx when you already manage certificates. This example assumes nginx runs
on the host with certificates in place and the sync server reachable on
`127.0.0.1:8787`.

```nginx
server {
    listen 80;
    server_name sync.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl;
    server_name sync.example.com;

    ssl_certificate     /etc/ssl/certs/sync.crt;
    ssl_certificate_key /etc/ssl/private/sync.key;

    # Require a login. Create the file with:
    #   htpasswd -c /etc/nginx/pergamon-sync.htpasswd youruser
    auth_basic           "pergamon sync";
    auth_basic_user_file /etc/nginx/pergamon-sync.htpasswd;

    location / {
        proxy_pass http://127.0.0.1:8787;

        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # Allow larger blob uploads to finish.
        proxy_read_timeout    120s;
        client_max_body_size  64m;
    }
}
```

Certificate renewal is your responsibility with this setup. If nginx runs as its
own container in the same Compose project, publish the sync server on the
internal Compose network only (no host `ports:` mapping) and set
`proxy_pass http://pergamon-sync:8787;` using the service name.

## Connecting a client

Once the server is reachable over HTTPS, point pergamon at it. Sync is opt-in and
client-initiated — a fresh, local-only install needs **no account and no key
ceremony**; you only create an account when you decide to sync. Onboarding is
split into **three explicit flows** so a device with existing data can't
silently make a duplicate account (ADR-029):

- **Create a new account** (first device) — `sync-device bootstrap`. Mints the
  account key, publishes this device, binds it to the server, and surfaces a
  **recovery code you must save**.
- **Attach an existing local account to a server** — `sync-remote enable`. Binds
  a local account you already have to a server. No new account key is created;
  it is a transport change only.
- **Join an existing account on a new device** — `sync-device enroll` +
  `accept` (SAS from a trusted device), or `sync-device recover` (recovery
  code). The new device receives the *existing* account key and never invents
  its own.

On the **first** device, create the account, then sync:

```sh
# First device: CREATE a new account and publish this device's identity.
# Prints a recovery code — write it down and store it offline.
pergamon sync-device bootstrap --server https://sync.example.com --account default

# Link the local database to that account + server and do the first round.
pergamon sync-remote enable --server https://sync.example.com --account default
pergamon sync-remote sync
```

If this device already has a local library (you used pergamon offline first),
`bootstrap` refuses unless you confirm with `--create-new-account`, so you don't
accidentally fork your data into a second account instead of joining the one you
already have elsewhere. A device that already belongs to an account is refused
outright — join with `enroll`/`recover` instead.

> **Save your recovery code.** `bootstrap` prints a high-entropy recovery code by
> default. It is the **only** way back into the account if you lose every
> device: **kafkade and the sync server cannot recover it for you** — the server
> only ever holds ciphertext. Store it offline. To create an account *without* a
> recovery code, pass `--no-recovery-code` (recovery stays off until you run
> `sync-device recovery-enable`); if you then lose every device, the account is
> unrecoverable. Set `PERGAMON_RECOVERY_PASSPHRASE` before `bootstrap` to wrap
> recovery under your own passphrase instead of a generated code.

Additional devices join through the onboarding flow (`sync-device invite` /
`enroll` / `approve` / `accept`, or `recovery-enable` / `recover`). See the
`pergamon sync-device --help` output,
[ADR-024](adr/024-device-onboarding-and-key-lifecycle.md), and
[ADR-029](adr/029-server-auth-identity-and-join-flows.md). The account key never
leaves your devices — the server only relays sealed artifacts.

### Authenticating through a reverse proxy

If your reverse proxy enforces authentication (as the Caddy and nginx
examples above do), give the client the credential through the **environment**
rather than embedding it in the server URL. The client sends it as an
`Authorization` header on every request, marks that header sensitive so it is
never logged, and — unlike a URL-embedded credential — never writes it to the
local sync state.

For HTTP Basic auth, export the username and password before running any
`pergamon sync-remote` or `pergamon sync-device` command:

```sh
export PERGAMON_SYNC_BASIC_USER=youruser
export PERGAMON_SYNC_BASIC_PASSWORD=your-password
pergamon sync-remote sync
```

For a bearer token instead, set `PERGAMON_SYNC_BEARER_TOKEN` (it takes
precedence over the Basic-auth variables when both are set):

```sh
export PERGAMON_SYNC_BEARER_TOKEN=your-token
pergamon sync-remote sync
```

The same variables also configure the `pergamon-server` background sync worker
when it syncs through the proxy.

Embedding the credential in the server URL still works as a last resort, but
is discouraged: it can leak into shell history, process listings, and logs.

## Data persistence

All persistent state lives under `/data` inside the container:

```text
/data/
└── pergamon-sync.db     SQLite database (encrypted envelopes + opaque blobs)
```

The server holds a single connection to this database, so under normal operation
there are no `-wal`/`-shm` sidecar files. The contents are **ciphertext and
blinded identifiers only** — the database is useless to anyone without your
account key.

The Compose file mounts a named volume (`pergamon-sync-data`) at `/data`, so data
survives `down`/`up`. The image deliberately does **not** declare a `VOLUME`
directive — you control where the data lives.

### Bind mounts and permissions

If you bind-mount a host directory instead of a named volume, that directory
must be writable by **UID 1000** — the unprivileged `pergamon` user the container
runs as:

```sh
mkdir -p ./sync-data && sudo chown 1000:1000 ./sync-data
docker run -d --name pergamon-sync \
  -p 8787:8787 \
  -v "$PWD/sync-data:/data" \
  pergamon-sync-server:latest
```

If the UID does not match, the container fails to open or create the database
with a permission error — see [Troubleshooting](#troubleshooting).

## Backup and restore

The sync database is **not** the source of truth — every client keeps a full
local copy — so a lost server database is recoverable by re-syncing from a
device. Backing it up is still worthwhile to avoid a full re-upload.

Because the store is ciphertext-only and there is no CLI in this image, backup is
**volume-level** and requires stopping the container so the database file is
consistent:

```sh
docker compose -f docker-compose.sync-server.yml stop
docker run --rm \
  -v pergamon-sync-data:/data:ro \
  -v "$PWD":/backup \
  debian:bookworm-slim \
  tar czf /backup/pergamon-sync-$(date +%F).tar.gz -C /data .
docker compose -f docker-compose.sync-server.yml start
```

Restore by extracting the archive back into the volume while the container is
stopped:

```sh
docker compose -f docker-compose.sync-server.yml stop
docker run --rm \
  -v pergamon-sync-data:/data \
  -v "$PWD":/backup \
  debian:bookworm-slim \
  sh -c 'rm -f /data/* && tar xzf /backup/pergamon-sync-YYYY-MM-DD.tar.gz -C /data'
docker compose -f docker-compose.sync-server.yml start
```

## Upgrading

1. Pull or rebuild the image:

   ```sh
   docker compose -f docker-compose.sync-server.yml pull        # published image
   # or, when building locally:
   docker compose -f docker-compose.sync-server.yml build --pull
   ```

2. Recreate the container:

   ```sh
   docker compose -f docker-compose.sync-server.yml up -d
   ```

Schema is initialized automatically on startup, and the data volume persists
across container replacements. Watch the logs to confirm a clean start:

```sh
docker compose -f docker-compose.sync-server.yml logs -f pergamon-sync
```

Pinning a specific version tag (rather than `latest`) makes upgrades deliberate
and reproducible. Because clients remain the source of truth, an upgrade that
resets the server database is recoverable by re-syncing.

## Troubleshooting

### Container won't start / keeps restarting

Check the logs — startup errors (bad config, unwritable data dir) are printed
there:

```sh
docker compose -f docker-compose.sync-server.yml logs pergamon-sync
```

Raise verbosity temporarily with `RUST_LOG=debug`.

### Permission denied on the data directory

Symptom: the log shows a failure to open or create `/data/pergamon-sync.db`.
This almost always means a bind-mounted host directory is not writable by
UID 1000:

```sh
sudo chown -R 1000:1000 ./sync-data
```

Named volumes (the default) don't have this problem because Docker initializes
their ownership from the image.

### Health check failing / container marked `unhealthy`

`docker ps` shows the health status in the `STATUS` column. A failing check
usually means the server isn't serving `GET /health` with HTTP 200. The endpoint
returns 503 if the store lock is poisoned (a prior panic) — restart the
container, and if it persists check the logs.

Probe it manually from the host:

```sh
curl -i http://localhost:8787/health
```

The container's built-in probe (`pergamon-sync-server health-check`) uses the
same endpoint; no `curl`/`wget` is installed in the image.

### Clients can't connect

- Confirm the server is reachable from the client host over HTTPS (through your
  reverse proxy), not just on `localhost`.
- If the proxy enforces authentication, make sure the client has valid
  credentials configured via `PERGAMON_SYNC_BASIC_USER` /
  `PERGAMON_SYNC_BASIC_PASSWORD` (or `PERGAMON_SYNC_BEARER_TOKEN`); see
  [Authenticating through a reverse proxy](#authenticating-through-a-reverse-proxy).
- Check the proxy's `client_max_body_size` / upload limits if large blob pushes
  fail.

## Security considerations

- **The server has no built-in authentication.** Anyone who can reach the port
  can push/pull encrypted blobs for any account. Never expose it directly to the
  public internet. For any non-local access, place it behind a reverse proxy
  that enforces **both** TLS and authentication (the Caddy and nginx examples
  above show HTTP Basic auth), or keep it on a trusted private network.
- **End-to-end encryption protects content, not availability.** The server only
  ever stores ciphertext and blinded identifiers and cannot read your data — but
  encryption alone does not stop an unauthenticated party from consuming storage
  or disrupting sync. That is what the auth layer is for.
- **Always use TLS for non-local access.** The server is plain HTTP; terminate
  TLS at the proxy and redirect HTTP to HTTPS.
- **Keep secrets out of version control.** Reference proxy passwords/hashes from
  a `.env` file (Compose reads `.env` automatically) and add `.env` to
  `.gitignore`.
- **Non-root by default.** The container runs as an unprivileged user (UID/GID
  1000), limiting the blast radius if the process is compromised. Don't override
  this with `--user 0` without a specific, understood reason.
- **Keep the image current.** Rebuild/pull periodically to pick up base-image
  security updates.

## Licensing (AGPL-3.0)

`pergamon-sync-server` is licensed under the **GNU Affero General Public License
v3.0 (AGPL-3.0-only)** — unlike the rest of pergamon, which is Apache-2.0 (see
[ADR-008](adr/008-licensing-apache-20-agpl-30.md)). The container image ships
this AGPL binary, and the image is labelled
`org.opencontainers.image.licenses=AGPL-3.0-only`. A copy of the license text is
included in the image at `/usr/share/doc/pergamon-sync-server/LICENSE`.

The AGPL's network-use clause (section 13) means that **if you run a modified
version of the server and let users interact with it over a network, you must
offer those users the corresponding source code of your modified version.**
Running the unmodified upstream image imposes no additional obligation beyond
the AGPL's normal terms. If you modify the server, publish your source (for
example, a public fork) and make it available to your users.

---

For the underlying design decisions (multi-stage build, non-root user, blind
relay model, health check, AGPL image), see
[ADR-026: Sync Server Deployment](adr/026-sync-server-deployment.md).
