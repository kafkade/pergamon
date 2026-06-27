# Self-hosting pergamon with Docker

This guide gets the pergamon web server (`pergamon-server`) running in a
container with persistent storage. It covers the quick start, configuration,
and putting it behind a reverse proxy for TLS.

> For the design rationale behind this packaging, see
> [ADR-018: Docker Deployment](adr/018-docker-deployment.md).

## Quick start

The fastest path is Docker Compose, which builds the image and creates a named
volume for your data:

```sh
docker compose up -d
```

The server is then reachable at <http://localhost:3000>. Check it is healthy:

```sh
curl http://localhost:3000/health
# {"status":"ok","version":"..."}
```

View logs and follow startup (migrations run automatically):

```sh
docker compose logs -f pergamon
```

Stop it (your data is preserved in the `pergamon-data` volume):

```sh
docker compose down
```

### Building and running without Compose

```sh
docker build -t pergamon:latest .

docker run -d --name pergamon \
  -p 3000:3000 \
  -v pergamon-data:/data \
  pergamon:latest
```

## Data persistence

All persistent state lives under `/data` inside the container:

```text
/data/
├── pergamon.db          SQLite database (content, sessions, config)
└── exports/             Backup export staging area
```

The Compose file mounts a named volume (`pergamon-data`) at `/data`, so data
survives `docker compose down && docker compose up`. The image deliberately does
**not** declare a `VOLUME` directive — you control where the data lives.

If you bind-mount a host directory instead of a named volume, that directory
must be writable by UID 1000 (the unprivileged `pergamon` user the container
runs as):

```sh
mkdir -p ./data && sudo chown 1000:1000 ./data
docker run -d -p 3000:3000 -v "$PWD/data:/data" pergamon:latest
```

## Configuration

Configuration is via environment variables. The image ships with
container-friendly defaults (binds `0.0.0.0:3000`, data in `/data`).

| Variable | Default (in image) | Description |
|---|---|---|
| `PERGAMON_HOST` | `0.0.0.0` | Address to bind to |
| `PERGAMON_PORT` | `3000` | Port to listen on |
| `PERGAMON_DATA_DIR` | `/data` | Directory for the database and exports |
| `PERGAMON_DB` | `$PERGAMON_DATA_DIR/pergamon.db` | Explicit database file path (overrides `DATA_DIR`) |
| `RUST_LOG` | `info` | Log filter (`error`, `warn`, `info`, `debug`, `trace`) |

Set them in `docker-compose.yml` under `environment:` or pass `-e` flags to
`docker run`.

## Health check

The image defines a `HEALTHCHECK` that runs the binary's built-in subcommand —
no `curl` or `wget` is installed in the runtime image:

```sh
pergamon-server health-check --url http://127.0.0.1:3000/health
```

It exits `0` when the endpoint returns HTTP 200, non-zero otherwise. Inspect the
container's health with `docker ps` (the `STATUS` column shows `healthy`).

## Reverse proxy (TLS)

The server speaks plain HTTP. For internet-facing deployments, terminate TLS at
a reverse proxy. Do **not** expose the HTTP port directly to the public internet.

### Caddy

Caddy provisions and renews certificates automatically. A minimal `Caddyfile`:

```text
pergamon.example.com {
    reverse_proxy pergamon:3000
}
```

Example Compose snippet placing pergamon behind Caddy (the pergamon service no
longer publishes its port directly):

```yaml
services:
  pergamon:
    build: .
    volumes:
      - pergamon-data:/data
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

### nginx

A minimal server block (assuming certificates are already in place):

```nginx
server {
    listen 443 ssl;
    server_name pergamon.example.com;

    ssl_certificate     /etc/ssl/certs/pergamon.crt;
    ssl_certificate_key /etc/ssl/private/pergamon.key;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

## Backups

The `pergamon` CLI binary is included in the image, so you can create a
self-contained backup while the server runs:

```sh
docker exec pergamon pergamon export backup /data/exports/backup.zip
docker cp pergamon:/data/exports/backup.zip ./backup.zip
```

To restore, stop the server first, then import:

```sh
docker compose stop
docker compose run --rm pergamon pergamon import backup /data/exports/backup.zip
docker compose start
```

## Upgrading

```sh
docker compose build --pull   # or: docker compose pull, for published images
docker compose up -d
```

Schema migrations run automatically on startup; the data volume persists across
container replacements.

---

A more comprehensive self-hosting guide (orchestration, monitoring, hardening)
is tracked separately. This document covers the essentials to get running.
