# ADR-007: Platform Boundaries

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

pergamon is intentionally multi-platform, but not all platforms need the same code or the same responsibilities. The project includes a Rust CLI/TUI, SQLite persistence, feed parsing, extraction, import/export tooling, an Obsidian plugin in TypeScript, a future iOS application via UniFFI, and possible future web surfaces via WASM or Axum. Without explicit platform boundaries, responsibilities would quickly blur and the architecture would become difficult to maintain.

The most important boundary concern is keeping the domain reusable while preventing networking and external integration logic from leaking everywhere. If each crate or client is allowed to fetch URLs, parse feeds, and extract article content independently, pergamon will accumulate duplicated logic, inconsistent behavior, and security drift. A solo-developed project especially benefits from a single, strongly enforced network edge.

There is also a language boundary. Rust should own core logic, storage, ingestion, and import/export behavior. TypeScript should be limited to the Obsidian integration surface. Swift should consume Rust functionality through UniFFI rather than reimplementing core rules. Future web work should be explicitly deferred until there is more clarity on whether WASM or server rendering is the correct direction.

The architecture therefore needs named crates, clear responsibilities, and a rule about where HTTP is allowed.

## Decision

pergamon will use the following crate and platform structure:

Rust crates:

- `pergamon-core` for pure domain logic
- `pergamon-storage` for SQLite persistence
- `pergamon-feed` for feed parsing
- `pergamon-extract` for article and metadata extraction
- `pergamon-import` for importers
- `pergamon-export` for exporters
- `pergamon-cli` as the binary entry point and orchestration layer

Other platforms:

- TypeScript in `apps/obsidian-plugin/` for the Obsidian plugin
- Swift via UniFFI for a future iOS app
- Web via WASM or Axum-rendered UI is explicitly deferred to a later ADR

Architectural rule: `pergamon-cli` is the only crate that performs HTTP. Feed fetching and article fetching happen there. Raw bytes are then passed to `pergamon-feed` and `pergamon-extract` for parsing and transformation. Those crates may parse bytes but do not perform network access themselves.

## Consequences

### Positive

- Creates explicit ownership for domain, storage, parsing, extraction, and UI orchestration.
- Prevents duplicated networking logic across crates.
- Keeps Rust core logic reusable by CLI, TUI, iOS, and future web surfaces.
- Makes the Obsidian plugin a focused integration layer rather than an alternative backend.
- Supports clearer licensing boundaries, especially around future server code.

### Negative

- Requires explicit orchestration code in `pergamon-cli` to connect all crates.
- Some developers may find the “only CLI does HTTP” rule stricter than necessary.
- Future non-CLI clients may need dedicated orchestration layers that mirror CLI behavior.
- Deferred web architecture means some future work remains intentionally unresolved.

## Rejected Alternatives

- **Allow each integration crate to fetch over HTTP as needed**: rejected because it duplicates network behavior and weakens architectural control.
- **Combine feed, extraction, and storage into one large application crate**: rejected because it reduces reuse and makes boundaries unclear.
- **Let the Obsidian plugin implement core ingestion logic in TypeScript**: rejected because domain and ingestion rules belong in Rust for consistency.
- **Decide the web architecture now**: rejected because the project does not yet need to commit to WASM versus server-rendered UI.
