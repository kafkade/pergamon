# syntax=docker/dockerfile:1

# ----------------------------------------------------------------------
# Builder stage: compile the web server and CLI binaries in release mode.
# The Rust version matches the workspace pin in rust-toolchain.toml.
# ----------------------------------------------------------------------
FROM rust:1.96-bookworm AS builder

WORKDIR /build

# Copy the full workspace. BuildKit cache mounts below keep the compiled
# dependency graph (cargo registry + target dir) warm across rebuilds, so a
# source-only change does not recompile every dependency.
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    set -eux; \
    cargo build --release -p pergamon-server -p pergamon-cli; \
    cp target/release/pergamon-server /usr/local/bin/pergamon-server; \
    cp target/release/pergamon /usr/local/bin/pergamon

# ----------------------------------------------------------------------
# Runtime stage: minimal Debian image with only the binaries and CA certs.
# SQLite is statically linked via rusqlite/bundled, so no libsqlite3 needed.
# ----------------------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as an unprivileged user (UID/GID 1000).
RUN groupadd -g 1000 pergamon \
    && useradd -u 1000 -g pergamon -m -s /usr/sbin/nologin pergamon

COPY --from=builder /usr/local/bin/pergamon-server /usr/local/bin/pergamon-server
COPY --from=builder /usr/local/bin/pergamon /usr/local/bin/pergamon

# Data directory for the SQLite database and exports, owned by the app user.
# Mount a volume here in Compose / `docker run` for persistence.
RUN mkdir -p /data && chown pergamon:pergamon /data

USER pergamon

EXPOSE 3000

# Bind to all interfaces inside the container (the binary defaults to
# 127.0.0.1 for native installs). Data lives under /data.
ENV PERGAMON_HOST=0.0.0.0 \
    PERGAMON_PORT=3000 \
    PERGAMON_DATA_DIR=/data \
    RUST_LOG=info

# Built-in health probe avoids needing curl/wget in the image.
HEALTHCHECK --interval=30s --timeout=5s --retries=3 --start-period=10s \
    CMD ["pergamon-server", "health-check", "--url", "http://127.0.0.1:3000/health"]

CMD ["pergamon-server"]
