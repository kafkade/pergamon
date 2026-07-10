# ADR-027: Browser Extension Architecture

**Status**: Accepted  
**Date**: 2026-07-09  
**Deciders**: kafkade

## Context

Epic #36 (moonshot features) adds a browser extension for one-click capture: the
user is reading a page in their desktop browser and wants to save it — or a
selection from it — into pergamon without leaving the page. This ADR defines the
extension's architecture and is the first of three issues in the browser-
extension track. It blocks #134 (the local capture API endpoint) and #135 (the
extension implementation), which own concrete endpoint paths, payload schemas,
and UI.

pergamon is local-first (ADR-001, ADR-016): the canonical store is the user's
own SQLite database, reached through a local process — either the self-hosted
`pergamon-server` (Axum, ADR-016/017/018) or the CLI. The extension must not
weaken that ownership model. In particular, the multi-device sync stack
(ADR-022, ADR-026) is a **blind relay** that stores only client-encrypted
ciphertext; it has no plaintext and no ingestion pipeline, so it is structurally
incapable of accepting a "save this URL" request. Capture therefore has to reach
a local pergamon instance, not the cloud.

pergamon already has exactly one ingestion pipeline, exercised by the CLI `save`
command and reused by web and by the iOS share extension (ADR-021):

1. Fetch the URL (HTTP lives outside `pergamon-core`, ADR-007).
2. Canonicalize via `pergamon_extract::canonicalize_url` (strip tracking params,
   normalize scheme/host/port, drop fragment, sort query).
3. Dedupe via `get_content_item_by_url(canonical)` against the unique-URL index.
4. Create a `ContentItem` (`status = Inbox`, UUID id, UTC timestamps) with
   `BookmarkMeta`, attaching a `Highlight` when a selection is present.
5. Extract to an `Article` when readability succeeds, otherwise keep it a
   `Bookmark`, upgrading in place (ADR-010).

The extension must feed **that same pipeline** rather than introduce a parallel
one. This ADR decides the three things the issue scopes: the WebExtension
structure and permissions, the transport to a local endpoint, and the auth /
pairing model. It builds on ADR-007 (HTTP outside the core), ADR-010 (unified
content model), ADR-016 (the local Axum server is the network entry point),
ADR-017 (single-owner auth and the deferred API-token design), ADR-021 (the
ingestion contract shared by every capture surface), and ADR-022/ADR-026 (sync
is a blind relay, out of the capture path).

Constraints that shape the decision:

- **Solo developer.** One extension codebase for all browsers; no per-browser
  fork and no heavyweight framework/build pipeline.
- **Local-first ownership.** Captures go to the user's own instance. No pergamon
  cloud account, no third-party capture service, no telemetry.
- **Least privilege.** A capture tool needs the current tab's URL, title, and
  optional selection on an explicit user gesture — not the ability to read every
  page the user visits or their browsing history.
- **Manifest V3 reality.** Chromium requires MV3; the extension background
  context is an ephemeral service worker, not a long-lived page, so no state may
  be assumed to survive between events.

## Decision

### One Manifest V3 WebExtension for Chromium and Firefox

The extension is a single **WebExtension** targeting **Manifest V3**, shipped to
both **Chromium-family browsers** (Chrome, Edge, Brave, Arc, Vivaldi) and
**Firefox**. Manifest V3 is mandatory on Chromium and supported on Firefox, so
one manifest and one code path cover the target set. Safari is explicitly
deferred: on macOS/iOS the native share extension (ADR-021) already provides
one-tap capture, so a Safari App Extension is not a priority and is left to a
follow-up if demand appears.

Where the two engines genuinely diverge, the extension uses the standard
`browser.*` promise-based API (via a thin `webextension-polyfill` shim over
Chromium's callback `chrome.*`) and keeps browser-specific manifest keys minimal
and feature-detected. There is no build step that produces materially different
behavior per browser.

### Structure

```text
┌──────────────────────────── Browser ────────────────────────────┐
│                                                                  │
│  Toolbar action (popup)     Context menu       Keyboard shortcut │
│        │                        │                      │         │
│        └────────────┬───────────┴──────────────────────┘         │
│                     ▼                                            │
│         Background service worker (MV3, ephemeral)               │
│   · reads active tab URL/title (activeTab)                       │
│   · reads selection via one-shot scripting.executeScript         │
│   · holds no long-lived page access                              │
│   · manages the outbound queue in extension storage             │
│                     │ fetch() with Bearer token                  │
└─────────────────────┼────────────────────────────────────────────┘
                      │ HTTP over loopback (127.0.0.1 / configured host)
                      ▼
        pergamon local capture endpoint  (#134)
   (pergamon-server Axum, or a CLI-hosted local listener)
                      │
                      ▼
        the one ingestion pipeline (ADR-021):
        canonicalize → dedupe → create/enrich → extract
```

Components:

- **Background service worker** — the only component that talks to pergamon. It
  owns the capture queue and the paired-instance configuration.
- **Popup (toolbar action)** — the primary one-click surface: shows the current
  page, a Save button, an inline tag field, and inbox/status feedback.
- **Context menu** — "Save to pergamon" and "Save selection to pergamon".
- **Optional keyboard command** — a `commands` shortcut for save.
- **Options page** — pairing setup: the endpoint URL and the pairing token, plus
  a "Test connection" action.

There is **no persistent content script** injected across sites. Selection text
is read on demand with a single `scripting.executeScript` call scoped to the
active tab at the moment the user invokes capture.

### Permissions: narrow, capture-and-selection only

The manifest requests the minimum:

| Permission | Why | Explicitly not requested |
|------------|-----|--------------------------|
| `activeTab` | Read the active tab's URL/title on user gesture | `tabs` (all-tabs enumeration) |
| `scripting` | One-shot selection read on the active tab | persistent content scripts on `<all_urls>` |
| `contextMenus` | The right-click save entries | — |
| `storage` | Store endpoint config, token, and the retry queue locally | — |
| `commands` (optional) | Keyboard shortcut | — |
| **Host permission** | Only the configured local endpoint origin (e.g. `http://127.0.0.1:3000/*`), added narrowly or via optional/runtime host permission | `<all_urls>` host access |

The extension never requests `history`, `bookmarks`, `webNavigation`,
`webRequest`, or broad host permissions. It cannot see pages the user does not
explicitly capture. This is the whole point: a capture tool should read the tab
you are on when you click Save, and nothing else.

### Transport: local loopback capture endpoint, never the sync server

The extension sends captures over **HTTP to a local pergamon instance** — the
`pergamon-server` Axum listener (ADR-016) or a CLI-hosted local listener —
default-bound to **loopback** (`127.0.0.1`). The concrete route, method, and
payload schema are owned by #134; at the contract level:

- The request carries what the extension can cheaply obtain: the **raw URL**, the
  **page title**, an optional **selection**, and optional **tags** / target
  status. It does **not** fetch the page, run readability, or sanitize HTML —
  that is the server's job, exactly as with CLI and share-extension capture
  (ADR-007, ADR-021).
- The endpoint hands the payload to the **existing ingestion pipeline**
  (canonicalize → dedupe → create/enrich → deferred extract). The extension adds
  no new dedupe or content logic; the canonical URL remains the single dedupe key
  across all surfaces.
- Content-kind mapping mirrors ADR-021: URL → one `ContentItem` (Bookmark, later
  Article); URL + selection → the item plus a linked `Highlight`; selection-only
  → a standalone `Highlight`.

The **sync server is explicitly out of the capture path.** It is a blind relay
(ADR-022, ADR-026) holding only ciphertext; it has no plaintext, no ingestion
code, and no notion of "save this URL". Captures reach the cloud, if at all,
only later — after the local instance ingests them and the normal client-driven
sync encrypts and relays them. This keeps capture local-first by construction.

For users who run pergamon on another machine on their LAN, the endpoint host is
configurable, but the security posture below (token + Origin lock + HTTPS
expectation off-loopback) still applies, and loopback is the default and
recommended mode.

### Offline / failure behavior: queue and retry

Because the local instance may be down (laptop closed, server not running) when
the user clicks Save, the background worker **enqueues** each capture in
`storage.local` and attempts delivery with bounded exponential backoff, draining
the queue when connectivity returns. The user gets immediate "Queued" feedback;
a badge count surfaces pending items. Idempotency is preserved end-to-end by the
pipeline's canonical-URL dedupe, so a retried capture converges to the same item
rather than duplicating it (ADR-021). The queue is capped and surfaces a clear
error if a capture repeatedly fails to deliver.

### Auth / pairing: a local API token, loopback-scoped

The extension authenticates to the local instance with a **pairing token**,
reusing ADR-017's already-accepted (previously deferred) API-token design rather
than inventing a new scheme:

- **Token format** `pgm_<id>.<secret>`: a public `id` for direct lookup plus a
  high-entropy `secret`. The server stores the `id`, a **BLAKE3 hash** of the
  secret, an optional expiry, a human-readable label (e.g. "Browser extension –
  Firefox"), and a last-used timestamp. BLAKE3 (not Argon2id) is correct here:
  the secret is high-entropy random, not a human password (ADR-017).
- **Minting** happens on the user's own instance — a `pergamon` CLI command or
  the web settings page — and the token is shown **once**. The user pastes it
  into the extension's options page. There is no pergamon account and no
  third-party OAuth; pairing is a local, one-time, copy-paste handshake between
  two things the user already owns.
- **Presentation.** The extension sends `Authorization: Bearer pgm_<id>.<secret>`
  on every capture request. Bearer-token requests are exempt from the web app's
  CSRF synchronizer-token flow (they carry no ambient cookie; ADR-017), and are
  the capture surface's only credential — the extension never reuses the owner's
  browser session cookie.
- **Revocation.** Tokens are listed and revocable from the same local UI/CLI;
  revoking one instantly disables that extension install without affecting web
  logins or other tokens. Changing the owner password does not need to
  invalidate capture tokens (they are independent credentials), but revocation
  is always available.

### Server-side hardening for the capture endpoint

The local endpoint (#134) enforces, independent of the extension:

- **Loopback bind by default** (`127.0.0.1`), matching ADR-016's default posture.
- **Origin allow-list.** Only the extension's origin
  (`chrome-extension://<id>` / `moz-extension://<uuid>`) is accepted via CORS;
  the endpoint sets restrictive CORS headers and rejects other origins. This
  stops a random web page's JavaScript from POSTing to the loopback endpoint even
  though it is same-host.
- **Token required.** Missing/invalid/revoked tokens get `401`; the endpoint does
  not fall back to cookie auth for capture.
- **Rate limiting** consistent with ADR-017's login limiter, to bound abuse.
- **No secrets to the extension.** Responses return only what the extension needs
  to show status (accepted / duplicate / queued item id) — never library
  contents.

## Consequences

### Positive

- **Local-first is preserved by construction:** captures go to the user's own
  loopback instance and the sync relay never participates in ingestion.
- **One codebase, all target browsers:** a single MV3 WebExtension over the
  standard `browser.*` API covers Chromium and Firefox with no per-browser fork.
- **Least privilege:** `activeTab` + one-shot `scripting` + a single-origin host
  permission means the extension can read the page you explicitly save and
  nothing else — no history, no all-sites content script, no browsing surveillance.
- **No duplicated logic:** the endpoint reuses the exact CLI/web/share-extension
  ingestion pipeline, so canonicalization, dedupe, and enrichment behave
  identically across every capture surface.
- **Resilient capture:** the queue-and-retry model makes Save succeed instantly
  even when the local instance is momentarily unavailable, with dedupe keeping
  retries idempotent.
- **Reused, revocable auth:** the ADR-017 API-token design gives a per-install,
  independently revocable credential without a new auth system or a cloud account.

### Negative

- **Pairing is a manual step:** the user must run their local instance and paste
  a token once. This is friction compared to a cloud extension, and is the price
  of local-first ownership.
- **Capture needs the instance reachable eventually:** while the queue absorbs
  short outages, a user who never runs pergamon gets a growing pending queue and
  no saved items.
- **MV3 service-worker ephemerality:** the background context can be torn down
  between events, so all state (queue, config) must live in `storage`, adding
  care around lifecycle and re-hydration.
- **Off-loopback LAN use is sharper-edged:** allowing a non-loopback host means
  the token now travels the network, so those users must front the instance with
  HTTPS; the default loopback path avoids this.
- **Safari is not covered** by this extension; macOS/iOS users lean on the native
  share extension (ADR-021) until a Safari App Extension is justified.

## Rejected Alternatives

- **Extension talks to the sync server / a pergamon cloud service.** Rejected:
  the sync relay is blind (ciphertext only, ADR-022/ADR-026) and cannot ingest,
  and a cloud capture service would break local-first ownership and add an
  account and a trusted third party. Capture belongs on the local instance.

- **Extension writes directly to the SQLite database (e.g. via Native
  Messaging to a helper that opens the DB).** Rejected for the same reasons
  ADR-021 rejected it for the share extension: it couples the extension to the
  schema and migrations, invites cross-process write/lock races, and duplicates
  the ingestion pipeline. Going through the local HTTP endpoint keeps the
  extension schema-agnostic and reuses one pipeline.

- **Full fetch + readability extraction inside the extension.** Rejected: it
  duplicates the pipeline, fails offline, bloats the extension, and fights CORS
  and MV3 constraints. The extension captures a URL + selection; the server
  fetches and extracts (ADR-007, ADR-010, ADR-021).

- **Persistent content script injected on `<all_urls>`.** Rejected as excessive
  privilege for a capture tool and a needless privacy/attack surface. `activeTab`
  plus a one-shot `scripting.executeScript` on user gesture reads the selection
  without standing access to every page.

- **Unauthenticated loopback endpoint (rely on "it's only 127.0.0.1").**
  Rejected: any web page's JavaScript can attempt requests to loopback, and
  DNS-rebinding/CSRF-style tricks make host-only trust unsafe. A required Bearer
  token plus an extension-origin CORS allow-list is the minimum bar.

- **Reuse the owner's web session cookie for capture.** Rejected: it would tie
  the extension to an active browser login, pull it into the CSRF flow, and make
  revocation coarse. A dedicated, independently revocable API token is cleaner
  (ADR-017).

- **Manifest V2 (long-lived background page).** Rejected: MV2 is deprecated/
  removed on Chromium. Targeting MV3 is required for the primary browser family
  and is supported on Firefox, so one MV3 codebase is the pragmatic choice.

- **Separate per-browser extensions.** Rejected for a solo developer: the
  `browser.*` API plus a polyfill lets one codebase serve Chromium and Firefox;
  maintaining forks is unjustified maintenance cost.
