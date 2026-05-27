# ADR-001: Zero-I/O Core Library

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

pergamon is a local-first personal information ingestion system with multiple front ends and future targets: CLI, TUI, Obsidian integration, iOS via UniFFI, and web via WASM or server rendering. The project DNA requires a Rust-first architecture with a cleanly separated core that can be reused across platforms. At the same time, pergamon must handle many integration concerns: HTTP fetching, feed parsing, HTML extraction, SQLite persistence, filesystem asset storage, and sync.

If these concerns are mixed into the core domain crate, the result is tighter coupling, harder testing, weaker portability, and reduced confidence in business logic changes. A core crate that depends on reqwest, rusqlite, filesystem APIs, or platform-specific libraries would be harder to compile to WASM, harder to expose via UniFFI, and harder to reason about in isolation. It would also make the core state machine and spaced repetition logic depend on external systems that are irrelevant to the domain model itself.

This decision is also intended to keep pergamon aligned with the kafkade project family, especially ldgr-core, tock-core, and toku-core. Those projects treat domain logic as pure computation and push integration complexity to edges. pergamon needs the same boundary if it is to remain understandable and maintainable as a solo-developed open source project.

The main domain responsibilities that belong in pergamon-core are stable and computational: domain models, content lifecycle state transitions, spaced repetition calculations, tag and collection rules, deduplication logic, and search/filter evaluation over already-loaded data. These can all be expressed without network access, filesystem access, or database drivers.

## Decision

pergamon-core will be a zero-I/O library. It will contain only domain logic and pure computation. It will not depend on reqwest, rusqlite, filesystem APIs, platform SDKs, or any crate that introduces networking, storage, or platform-bound behavior.

pergamon-core will own:
- domain models
- content state machine
- spaced repetition engine
- tag and collection management rules
- deduplication logic
- search and filtering logic

All I/O will live outside the core:
- HTTP fetching in pergamon-cli via reqwest
- SQLite persistence in pergamon-storage via rusqlite
- feed parsing in pergamon-feed
- extraction in pergamon-extract

Core APIs will accept and return plain Rust data structures and errors suitable for deterministic testing and reuse across CLI, TUI, UniFFI, and WASM targets.

## Consequences

### Positive
- Enables WASM compilation and future browser reuse.
- Makes core behavior easy to unit test with pure functions and fixtures.
- Preserves clean platform boundaries for CLI, iOS, and future web clients.
- Reduces accidental coupling between domain rules and integration details.
- Keeps pergamon consistent with ldgr-core, tock-core, and toku-core patterns.

### Negative
- Requires more translation code between integration crates and core models.
- Some convenience is lost because network or storage helpers cannot be called directly from domain code.
- More crate boundaries mean more API design work up front.
- Parsing and persistence workflows may require explicit mapping layers.

## Rejected Alternatives

- **Put HTTP and SQLite directly in pergamon-core**: rejected because it breaks portability, makes testing heavier, and weakens architectural boundaries.
- **Allow “small exceptions” for filesystem or platform APIs in core**: rejected because boundary erosion would happen quickly and make the rule meaningless.
- **Move only some domain logic into core and leave the rest in the CLI**: rejected because it would duplicate logic across interfaces and reduce reuse.
- **Use traits in core backed by runtime I/O implementations**: rejected for initial architecture because even abstracted I/O tends to leak concerns into domain design and is unnecessary for pergamon’s current needs.
