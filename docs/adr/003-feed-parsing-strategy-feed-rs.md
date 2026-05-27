# ADR-003: Feed Parsing Strategy — feed-rs

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

A core responsibility of pergamon is replacing Inoreader-style subscription and ingestion workflows. That means consuming multiple feed formats found in the wild, not just clean RSS 2.0. Real feeds may be Atom 1.0, JSON Feed, or malformed variants with inconsistent encoding, namespace handling, and date formatting. The parser choice affects reliability, maintenance burden, and how much normalization logic pergamon must own.

pergamon also has strong architectural constraints. The core crate must stay zero-I/O, while parsing of external formats belongs in integration-oriented crates. Feed fetching will happen in pergamon-cli via HTTP, but raw bytes still need to be parsed into a normalized internal representation before they can be persisted or passed into pergamon-core workflows.

Using separate crates for RSS and Atom would introduce multiple external models, more conditional logic, and more custom mapping code. Supporting JSON Feed in that design would require yet another parser and more glue. A custom parser is even less attractive: it would add substantial surface area, create maintenance burden for a solo developer, and likely perform worse on the long tail of malformed feeds than a mature library.

pergamon needs one parser API that can accept bytes from the CLI, normalize the common concepts, and keep protocol-specific complexity out of the rest of the system.

## Decision

pergamon will standardize feed parsing on the `feed-rs` crate.

`feed-rs` will live in the `pergamon-feed` crate, not `pergamon-core`. This keeps external format handling outside the pure domain layer, while still allowing pergamon-cli to remain the only crate that performs HTTP. The flow will be:
1. `pergamon-cli` fetches raw bytes with reqwest.
2. Raw bytes are passed to `pergamon-feed`.
3. `pergamon-feed` uses `feed-rs` to parse and normalize the feed.
4. The normalized result is mapped into pergamon’s internal models for storage and domain processing.

This approach supports RSS 2.0, Atom 1.0, and JSON Feed through a unified parsing interface. It also leverages `feed-rs`’s existing normalization and format-handling behavior instead of rebuilding it in project code.

## Consequences

### Positive
- Supports RSS, Atom, and JSON Feed through a single library and API.
- Reduces glue code and model conversion complexity.
- Improves resilience against real-world feed inconsistencies and encoding issues.
- Keeps pergamon-core free from external format concerns.
- Makes future feed-related features easier to add in one integration crate.

### Negative
- Adds a dependency whose parser behavior pergamon does not fully control.
- Normalization performed by `feed-rs` may hide some source-specific details unless explicitly preserved.
- If upstream maintenance quality changes, pergamon may need to adapt or fork.
- Some format edge cases may still require post-parse cleanup in pergamon-feed.

## Rejected Alternatives

- **Use `rss` plus `atom_syndication`**: rejected because it introduces two crates, two models, and more mapping code, with JSON Feed still unaddressed.
- **Write a custom parser**: rejected because it is unnecessary, high-maintenance, and unlikely to outperform a mature library on real feed diversity.
- **Parse feeds in pergamon-core**: rejected because external format handling does not belong in the zero-I/O domain crate.
- **Let pergamon-cli parse feeds directly**: rejected because parsing should be isolated in a reusable crate rather than embedded in the binary layer.
