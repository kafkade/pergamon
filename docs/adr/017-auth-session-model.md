# ADR-017: Auth and Session Model for Web App

**Status**: Accepted  
**Date**: 2026-05-29  
**Deciders**: kafkade

## Context

ADR-016 decided that the Phase 5 web interface is an Axum server rendering HTML with HTMX. The web app is a single-user, self-hosted companion deployed via Docker. It does not need multi-user accounts, OAuth federation, or cloud-managed identity.

However, because the server is network-exposed, it still requires:

- Protection against unauthorized access to the personal library.
- Session management for stateful browser interactions across page loads and HTMX partial updates.
- A simple, secure setup experience that fits within the Phase 5 acceptance criterion of under 10 minutes from Docker start to working web UI.

The auth model must also work behind common reverse proxies (nginx, Caddy, Traefik) since most self-hosted deployments terminate TLS at a proxy rather than at the application.

Single-user means there is no user table, no roles, no permission model. There is one owner and one password.

## Decision

### Auth mechanism: single password

The web app is protected by a single owner password. There is no username. The login form presents a password field only. For password manager compatibility, the form includes a hidden or read-only username field with the fixed value `owner`.

### Password hashing: Argon2id

The owner password is hashed using Argon2id via the `argon2` Rust crate. Argon2id is the current OWASP recommendation for password hashing. It resists GPU-based and side-channel attacks through memory-hard computation.

Default parameters follow OWASP guidance. The hash is stored in the `web_config` key-value table in the pergamon SQLite database.

### Session storage: server-side sessions in SQLite

Authenticated browser sessions are stored server-side in a `web_sessions` table in the existing pergamon SQLite database.

Session design:

- **Session ID:** 256-bit cryptographically random value, generated via a CSPRNG.
- **Storage:** Only a BLAKE3 hash of the session ID is stored in the database. The raw session ID is never persisted.
- **Cookie:** The raw session ID is sent to the browser in a cookie with these attributes: `HttpOnly`, `SameSite=Lax`, `Path=/`, and `Secure` when HTTPS is configured.
- **Expiry:** Sessions expire after a configurable idle timeout (default: 7 days). Expired sessions are deleted on access and by periodic cleanup.
- **Rotation:** The session ID is rotated immediately after successful login to prevent session fixation.
- **Logout:** Deleting the session row and clearing the cookie. A "log out all sessions" action deletes all rows from the sessions table.
- **Invalidation:** Changing the owner password invalidates all existing sessions.

Why not JWT: for a single-user server-rendered application, JWT adds token refresh complexity and revocation difficulty without commensurate benefit. Server-side sessions are simpler, natively revocable, and do not require the browser to manage token lifecycle.

### CSRF protection: synchronizer token pattern

State-changing requests (POST, PUT, DELETE) require a valid CSRF token.

- A per-session CSRF token is generated when the session is created.
- HTML forms include the token as a hidden `_csrf` field.
- HTMX requests include the token via a global `hx-headers` meta configuration on the page, so all HTMX-initiated requests carry the token automatically.
- The server validates the CSRF token on every state-changing request authenticated via session cookie, including HTMX partial updates.
- The `HX-Request: true` header is not treated as a CSRF defense. It is useful for distinguishing full-page from fragment responses but is not a security boundary.
- The server also validates `Origin` or `Referer` headers on unsafe methods when present, as defense-in-depth.
- Requests authenticated via API bearer tokens (future) are exempt from CSRF validation because they do not use ambient cookie credentials.

### Rate limiting: keyed per-IP limiter

Login attempts are rate-limited to mitigate brute-force attacks.

- **Limit:** 5 failed login attempts per 60 seconds per client IP.
- **Implementation:** An in-memory keyed rate limiter (e.g., `governor` crate or a simple time-windowed map). The single-process architecture means in-memory state is sufficient.
- **Response:** HTTP 429 Too Many Requests with a `Retry-After` header when the limit is exceeded.
- **Client IP:** Determined from the socket address by default. When trusted-proxy mode is enabled (see below), the rightmost untrusted IP from `X-Forwarded-For` is used.
- **Scope:** Rate limiting applies to the login endpoint and the setup endpoint.

### Initial setup flow

The first-run setup must be secure even if the server is briefly reachable on a network before configuration is complete.

Three setup paths, in order of precedence:

1. **Environment variable (headless Docker).** Set `PERGAMON_PASSWORD_HASH` to a pre-computed Argon2id hash string. The server starts fully operational with no setup page. A CLI helper, `pergamon-web hash-password`, generates the hash interactively for use in Docker Compose files or environment configs.

2. **Setup token (interactive).** If no password hash is configured, the server starts in setup mode. It generates a one-time setup token and prints it to the server log. The setup page at `/setup` requires this token along with the new password. This prevents an unauthorized party who discovers the server from claiming ownership.

3. **Refuse to start.** If setup mode is explicitly disabled via `PERGAMON_DISABLE_SETUP=true` and no password hash is stored, the server exits with a clear error message explaining how to configure a password.

After initial setup is complete, the `/setup` endpoint is permanently disabled until the password hash is removed from the database.

```text
First-run setup (interactive):

  Server starts → no password hash found → generates setup token
  Server logs: "Setup token: abc123... Visit /setup to configure."

  Browser → GET /setup → form: [setup token] [new password] [confirm]
  Browser → POST /setup → validate token → hash password → store → redirect /login

First-run setup (headless):

  PERGAMON_PASSWORD_HASH=<hash> docker run pergamon-web
  Server starts → password hash found → setup disabled → serves /login

Login:

  Browser → GET /login → form: [password]
  Browser → POST /login → verify Argon2id → create session → set cookie → redirect /

Authenticated request:

  Browser → GET /inbox (cookie) → lookup hashed session ID → valid → render page

Logout:

  Browser → POST /logout → delete session → clear cookie → redirect /login
```

### API tokens: design accepted, implementation deferred

For future programmatic access (scripts, CLI-to-web integration, automation), the following design is accepted but not required for Phase 5 MVP:

- API endpoints accept `Authorization: Bearer <token>` in addition to session cookies.
- Tokens use a structured format with a public prefix for lookup: `pgm_<id>.<secret>`.
- The server stores the token ID, a BLAKE3 hash of the secret portion, an optional expiry, a human-readable label, and a last-used timestamp.
- BLAKE3 is used instead of Argon2id because API tokens are high-entropy random secrets that do not benefit from expensive memory-hard hashing. The token ID enables direct database lookup without scanning all hashes.
- Tokens are shown to the user once at creation and cannot be retrieved afterward.
- Tokens are managed via the web UI settings page or a CLI command.

Implementation is deferred until programmatic API endpoints are added. The session and password infrastructure in Phase 5 does not depend on this.

### HTTPS and TLS: reverse proxy recommended

pergamon-web runs plain HTTP by default, listening on `127.0.0.1:3000`.

- **Production deployments** should terminate TLS at a reverse proxy (Caddy, nginx, Traefik). Documentation will include sample configurations for Caddy and nginx.
- **Secure cookies:** The `Secure` cookie flag is set only when HTTPS is detected. Detection requires explicit configuration, not automatic header inspection (see proxy trust below).
- **Direct exposure:** Running pergamon-web directly on the public internet without TLS is unsupported and undocumented.

### Reverse proxy trust: explicit opt-in

Forwarded headers (`X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`) are not trusted by default. An attacker who reaches the server directly could spoof these headers to manipulate rate limiting, cookie security, or redirect behavior.

Trust is enabled explicitly:

- `PERGAMON_TRUSTED_PROXIES` — a comma-separated list of CIDR ranges (e.g., `127.0.0.1/32,172.16.0.0/12`) from which forwarded headers are accepted.
- `PERGAMON_COOKIE_SECURE=true` — forces the `Secure` cookie flag regardless of detected protocol, for deployments where protocol detection is unreliable.
- When trusted-proxy mode is active, the rightmost untrusted IP in `X-Forwarded-For` is used for rate limiting, and `X-Forwarded-Proto` determines the `Secure` flag.

### Backup and restore considerations

Session and auth metadata are stored in the same SQLite database as content.

- **Full backup** (`export backup`) includes the password hash and API token metadata so that a restored instance retains its auth configuration.
- **Active sessions are excluded** from backups. Restoring a backup forces re-authentication.
- API tokens are included in backups to preserve programmatic access configuration.

## Consequences

### Positive

- Single-password model is the simplest possible auth for a single-user app. No user management, no roles, no federation.
- Server-side sessions are natively revocable, simple to implement, and require no client-side token management.
- Argon2id provides strong password protection against modern attack vectors.
- Setup token prevents unauthorized first-run password capture on network-exposed instances.
- Explicit proxy trust avoids header-spoofing attacks on improperly configured deployments.
- CSRF synchronizer tokens protect all state-changing requests, including HTMX partials.
- The deferred API token design avoids premature complexity while preserving a clear path for future programmatic access.

### Negative

- Single password means no audit trail of "which device" performed an action. Acceptable for single-user.
- Server-side sessions require periodic cleanup of expired rows.
- Setup token must be retrieved from server logs, which adds a step to the interactive setup flow.
- Explicit proxy trust configuration adds a setup step for reverse-proxy deployments, though this is documented with examples.
- CSRF tokens add a hidden field to every form and a global header to HTMX configuration, requiring template discipline.

## Rejected Alternatives

- **OAuth or OIDC federation:** Rejected because it introduces a cloud dependency and substantial integration complexity for a single-user self-hosted app.
- **Client certificate authentication (mTLS):** Rejected because certificate management is burdensome for the target user and not well-supported by all browsers and reverse proxy setups.
- **JWT-based sessions:** Rejected because JWTs complicate revocation and session management for a server-rendered single-user app. Server-side sessions are simpler and more appropriate.
- **No authentication (rely on network isolation):** Rejected because the server may be exposed to a local network or the internet, and the library contains personal data that must be protected by default.
- **Multi-user with roles:** Rejected because pergamon is a personal tool. Multi-user support is explicitly out of scope for the foreseeable roadmap.
- **Unauthenticated first-run setup page:** Rejected because a publicly reachable instance could be claimed by anyone who discovers the server before the owner configures it.
