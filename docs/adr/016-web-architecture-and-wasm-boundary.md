# ADR-016: Web Architecture and WASM Boundary

**Status**: Accepted  
**Date**: 2026-05-29  
**Deciders**: kafkade

## Context

pergamon's Phase 5 goal is to ship a self-hosted web interface for browsing, reading, searching, and reviewing the local library from any browser. The WASM spike (#28) confirmed that pergamon-core compiles to a 6.2 KB gzipped WASM module, proving that client-side domain logic in the browser is technically viable.

Three architectures were evaluated:

1. **Axum + server-rendered HTML + HTMX.** Server renders HTML pages and fragments. HTMX handles partial page updates without a JavaScript framework. No JS build pipeline required.

2. **Leptos.** Full Rust SSR plus client-side hydration. Single-language stack with reactive components, but requires trunk or cargo-leptos tooling and a less mature ecosystem.

3. **Axum + WASM core + thin TypeScript shell.** Server provides a JSON API. Client-side TypeScript or React renders the UI and uses WASM-compiled pergamon-core for domain logic. The roadmap's Decision #11 originally recommended this approach, marked [Validation Required].

Several constraints inform the decision:

- **Solo developer.** Every additional build tool, language, and framework adds maintenance burden. The web UI must not become a second full-time codebase.
- **Self-hosted, single-user.** The web app is deployed by the same person who uses it. Multi-user, multi-tenant, and high-availability concerns do not apply.
- **Server already runs Rust.** The Axum server has native access to pergamon-core, pergamon-storage, and all domain logic. Running that same logic again in the browser via WASM is redundant for a server-backed app.
- **Local-first.** The canonical data store is server-side SQLite. The browser is a view layer, not a storage layer. Content mutation requires server connectivity.
- **Docker deployment.** Phase 5 ships a single Docker image. Minimizing the build pipeline simplifies both CI and end-user deployment.

## Decision

### Architecture: Axum + server-rendered HTML + HTMX

The Phase 5 web application will be an Axum HTTP server that renders HTML on the server and uses HTMX for partial page updates. There is no JavaScript framework, no TypeScript build pipeline, and no client-side WASM in Phase 5.

### Component layout

```text
┌─────────────────────────────────────────────────────────────┐
│                        Browser                              │
│                                                             │
│  HTML pages + HTMX attributes + minimal inline JS           │
│  (no framework, no bundler, no WASM)                        │
│                                                             │
│  Requests: standard links, forms, HTMX partial updates      │
│  Responses: full HTML pages or HTML fragments                │
└──────────────────────────┬──────────────────────────────────┘
                           │ HTTP
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                   pergamon-web (Axum)                        │
│                                                             │
│  Route handlers ──→ application service layer               │
│  Askama templates ──→ compiled into binary                  │
│  Static assets ──→ embedded or served from disk             │
│  Session / auth middleware                                   │
└───────┬──────────┬──────────┬──────────┬────────────────────┘
        │          │          │          │
        ▼          ▼          ▼          ▼
  pergamon-   pergamon-   pergamon-   pergamon-
    core      storage       feed      extract
  (domain)   (SQLite)    (parsing)  (extraction)
```

### How the web app consumes storage

Axum route handlers call Rust application-layer functions that use pergamon-storage directly as a crate dependency. There is no REST API boundary between the web UI and the database. The browser communicates with the server over HTTP using standard links, forms, and HTMX requests. The server returns rendered HTML pages or HTML fragments, not JSON resources.

Domain invariants remain in pergamon-core. Storage operations remain in pergamon-storage. The web crate orchestrates these the same way pergamon-cli does.

### Template and rendering strategy

HTML templates use Askama, a compile-time, type-checked, Jinja2-like template engine. Templates are compiled into the server binary, eliminating runtime template file dependencies and catching template errors at build time.

Page structure:

- Full page loads return complete HTML documents with standard navigation.
- Interactive actions (triage, tag, archive, review response) use HTMX to swap HTML fragments without full page reloads.
- Forms submit via standard POST and work without JavaScript. HTMX enhances the experience but is not required for basic functionality.

### Asset pipeline and build tooling

No JavaScript build pipeline. No npm. No webpack, vite, or esbuild.

- **CSS:** A single stylesheet, possibly using a minimal framework such as Pico CSS. Served as a static file embedded in the binary or served from disk.
- **JavaScript:** HTMX loaded from a vendored copy with no CDN dependency. Any additional inline JS is minimal and progressive.
- **Icons and images:** Vendored SVG icons embedded or served as static assets.
- **Build:** `cargo build` produces the complete web application. No separate frontend build step.

### WASM boundary: Phase 5

In Phase 5, no pergamon-core functions are exposed to the browser via WASM. All domain logic runs server-side in native Rust.

| Concern | Phase 5 location | Rationale |
|---|---|---|
| FSRS scheduling | Server (native Rust) | Server owns review state in SQLite |
| Document state transitions | Server (native Rust) | Mutations require persistence |
| URL canonicalization and dedup | Server (native Rust) | Runs during ingestion, not in browser |
| Tag and collection validation | Server (native Rust) | Applied during save operations |
| Full-text search | Server (SQLite FTS5) | Index is server-side |
| Content rendering | Server (Askama templates) | HTML generated on server |
| Offline reading cache | Browser cache or service worker | Static assets and previously loaded pages only |
| Client-side domain logic | None | Not needed when server is available |

### WASM boundary: future candidates

If a future phase requires an offline-capable PWA or standalone client-side web app, the following pergamon-core functions are natural WASM export candidates. These are documented for architectural awareness and are non-binding.

- **FSRS review scheduling:** Pure computation over review state. Enables offline review sessions.
- **Content state machine:** Enables optimistic local state transitions with server reconciliation.
- **URL canonicalization:** Enables client-side duplicate checking before save.
- **Tag and collection validation rules:** Enables immediate input validation without a server round-trip.

The WASM spike confirmed that pergamon-core compiles to approximately 6.2 KB gzipped. A future ADR should define the exact WASM API surface, browser storage adapter (OPFS or IndexedDB), and client-server reconciliation strategy if this path is pursued.

### Progressive enhancement and offline story

The web UI provides progressive enhancement, not offline-first capabilities.

- **No-JS baseline.** Links navigate between full pages. Forms submit via standard POST. The core read, search, and browse experience works without JavaScript.
- **HTMX enhancement.** When JavaScript is available, HTMX provides smoother partial page updates, inline triage actions, and non-blocking form submissions.
- **Offline limitations.** Creating, editing, reviewing, and searching content all require server connectivity. The browser does not store a local copy of the database or run domain logic.
- **Static asset caching.** A service worker may cache the app shell (HTML layout, CSS, HTMX script, icons) for faster subsequent loads, but dynamic content requires the server.

True offline content mutation and sync would require a browser storage adapter and client-side WASM boundary. This is explicitly deferred beyond Phase 5.

### New crate: pergamon-web

A new `pergamon-web` crate will be added to the workspace.

- **License:** AGPL-3.0, consistent with the roadmap's guidance that network-facing server surfaces use AGPL to ensure improvements to self-hosted deployments remain open. This extends ADR-008's principle: Apache-2.0 for portable client and library crates, AGPL-3.0 for network-facing server components. The web UI is a network service and follows the same licensing boundary as the future sync server.
- **Responsibility:** Axum route handlers, Askama templates, static assets, session and auth middleware, and web-specific orchestration.
- **Not merged with pergamon-server.** The sync server handles encrypted blobs with a "server never sees plaintext" trust model. The web UI reads and renders plaintext library content. These are architecturally distinct services with different security properties and should remain separate crates.

### Impact on the crate dependency graph

```text
pergamon-web (new, AGPL-3.0)
├── pergamon-core (Apache-2.0)
├── pergamon-storage (Apache-2.0)
├── pergamon-feed (Apache-2.0)
├── pergamon-extract (Apache-2.0)
├── axum
├── askama
├── tower / tower-http
└── [session/auth crate TBD]

pergamon-server (future, AGPL-3.0, sync only)
├── pergamon-core (Apache-2.0)
├── axum
└── [encrypted blob storage]

pergamon-cli (existing, Apache-2.0)
├── pergamon-core
├── pergamon-storage
├── pergamon-feed
├── pergamon-extract
├── pergamon-import
├── pergamon-export
├── reqwest
├── ratatui
└── clap
```

The Apache-2.0 library crates remain unchanged. Only network-facing server crates use AGPL-3.0.

## Consequences

### Positive

- Eliminates the TypeScript and npm build pipeline entirely. One language, one build system, one binary.
- Server-side rendering reuses all existing Rust crates natively with no serialization boundary for domain logic.
- Askama templates are type-checked at compile time, catching template errors before deployment.
- HTMX provides an interactive feel without a JavaScript framework or client-side state management.
- Docker deployment is a single static binary with embedded templates and assets.
- Progressive enhancement means the UI works at baseline without JavaScript.
- Preserves WASM as a future option for offline-capable clients without committing to its complexity now.
- Separate `pergamon-web` crate keeps the web server cleanly isolated from CLI and sync server concerns.

### Negative

- Server-rendered HTML is less suitable for highly interactive client-side experiences such as drag-and-drop, real-time updates, or complex client-side state. This is accepted because Phase 5 does not require these.
- HTMX partial updates require careful route design to return both full pages and HTML fragments. Template composition needs discipline to avoid duplication.
- No offline content mutation. Users must have server connectivity to create, edit, or review content through the web interface.
- Askama compile-time templates require recompilation for template changes during development, though `cargo watch` mitigates this.
- The WASM boundary decision for client-side web apps is partially deferred and may require a follow-up ADR when offline web or PWA features are prioritized.

## Rejected Alternatives

- **Leptos (full Rust SSR + client hydration):** Rejected because the ecosystem is less mature, tooling complexity is higher (trunk or cargo-leptos), SSR hydration debugging is harder, and the solo developer would be locked into a framework with smaller community support. Leptos optimizes for rich client-side interactivity that Phase 5 does not require.

- **Axum + WASM core + thin TypeScript shell:** Rejected for Phase 5 because it introduces a TypeScript and npm build pipeline, a serialization boundary between client and server, and a two-language maintenance burden. The WASM spike (#28) proved this is technically feasible at 6.2 KB gzipped, but for a single-user server-backed web app, running domain logic in the browser is redundant. This architecture remains a valid future option if pergamon needs an offline-capable PWA or standalone client-side web app.

- **SPA with JSON REST API:** Rejected because it maximizes client-server separation at the cost of duplicating rendering logic, adding a full JavaScript framework, and requiring API versioning. A server-rendered approach is simpler for a solo developer shipping a self-hosted personal tool.

- **No web interface (keep CLI/TUI only):** Rejected because browser access from any device is a stated Phase 5 goal and expands pergamon's accessibility beyond terminal users.
