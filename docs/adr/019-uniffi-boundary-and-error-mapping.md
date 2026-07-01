# ADR-019: UniFFI Boundary and Error Mapping

**Status**: Accepted  
**Date**: 2026-07-01  
**Deciders**: kafkade

## Context

Phase 6 delivers a native SwiftUI iPhone app that reuses `pergamon-core` through
UniFFI instead of reimplementing domain logic in Swift (epic #34). The UniFFI
spike (#29, `docs/spikes/uniffi-ios-findings.md`) proved the path is technically
open: `pergamon-core` compiles to `aarch64-apple-ios` / `aarch64-apple-ios-sim`,
UniFFI generates idiomatic Swift bindings with no hand-written `unsafe` glue, a
SwiftUI app links the static library and runs in the Simulator, and the shipped
binary overhead is ~0.5 MB stripped. The `pergamon-uniffi` facade crate already
exists as the spike deliverable.

The spike deliberately kept its surface tiny (a few records, two mirrored enums,
five free functions serving an in-memory corpus) and explicitly deferred four
decisions to this ADR:

1. **The exported UniFFI surface** — which types, functions, and stateful
   handles cross the boundary, and how narrow to keep it.
2. **Error mapping** — how Rust `Result` / `thiserror` errors become Swift
   `throws`.
3. **Threading and async** — how calls behave when made from SwiftUI, and when
   to use UniFFI's `async fn` support.
4. **Binding ABI versioning** — how the generated bindings are versioned and
   regenerated.

The governing constraint is the Phase 6 risk mitigation, already validated by
the spike: **keep the exposed surface area narrow and wrapper-friendly.** A
narrow facade is what keeps both ergonomics and binary size healthy, and it
decouples the Apple-facing ABI from churn in internal crates. This ADR ratifies
the conventions the spike proved and decides the deferred questions. It is a
documentation decision; the reference implementation of the error enum and the
stateful `Library` handle lands with the bindings-hardening work (#113).

This ADR builds on ADR-007 (Swift consumes Rust through UniFFI rather than
reimplementing core rules) and mirrors ADR-016's treatment of the WASM boundary
for the web. It concerns the `pergamon-core` ↔ Swift boundary specifically.

## Decision

### Facade crate and binding mode

`pergamon-uniffi` is the **single, exclusive** UniFFI export surface for Apple
clients. No other crate is exported to Swift, and Swift never links
`pergamon-core`, `pergamon-storage`, or any internal crate directly.

- **Proc-macro mode, no `.udl`.** The contract lives in Rust next to the types
  via `#[uniffi::export]`, `#[derive(uniffi::Record / Enum / Error / Object)]`,
  and `uniffi::setup_scaffolding!()`. There is no separate interface definition
  file to keep in sync.
- **Vendored generator.** `uniffi-bindgen` ships as a `[[bin]]` of the facade
  crate so the generator is pinned to the exact `uniffi` runtime version,
  eliminating generator/runtime drift in CI and on developer machines.
- **Lint boundary.** `pergamon-uniffi` is the only crate that relaxes the
  workspace `unsafe_code = "forbid"` lint, because UniFFI's generated scaffolding
  emits `unsafe` FFI glue. `pergamon-core` stays zero-I/O and `forbid`-clean; the
  thin FFI crate absorbs the `unsafe` allowance.

### Exported surface

The facade exposes **product-shaped** types and a small number of handles, never
internal crate types. Internal types are converted to FFI types at the boundary
via `From` implementations that the facade owns.

- **Records** (`#[derive(uniffi::Record)]`): plain data views such as
  `ContentItem`, and the annotation/highlight and review-card shapes the app
  renders. Records are value types with no behavior.
- **Enums** (`#[derive(uniffi::Enum)]`): mirrored discriminators such as
  `ContentType` and `Status`, decoupled from the internal
  `pergamon_core::content_type::ContentType` / `status::DocumentStatus`.
  Mirroring is intentional: the FFI ABI must not shift every time an internal
  enum gains a variant that Apple does not yet render.
- **Object handles** (`#[derive(uniffi::Object)]`): stateful entry points such
  as a future `Library` that wraps the on-device SQLite store. Methods on the
  handle (`inbox()`, `item(id:)`, `search(query:)`, `record_review(...)`) are
  the primary way the app drives the core. The handle owns interior state behind
  `Send + Sync` synchronization; Swift holds it as a reference type.
- **Free functions** (`#[uniffi::export]`): a handful of stateless helpers such
  as `library_version()` and `reading_minutes(text:)`.

The rule of thumb: expose what the app screen needs, shaped the way the screen
consumes it. Do not expose repositories, connection objects, migration APIs, or
other internal machinery.

### Boundary type mapping

The facade owns these conversions so Swift never sees an awkward or unstable
type. This ratifies the mapping validated in the spike.

| Core type | FFI type | Swift result | Rationale |
|-----------|----------|--------------|-----------|
| `Uuid` | `String` | `String` | UUID is not a UniFFI primitive; strings are trivial and stable |
| `OffsetDateTime` | `i64` (epoch millis) | `Date` (one-line app-side map) | Avoids exposing a time library across the ABI |
| `ContentType` / `DocumentStatus` | mirrored `enum` | native Swift `enum` | Decouples the FFI ABI from internal enums |
| `Option<T>` | `Option<T>` | Swift optional | Native |
| `Vec<T>` | `Vec<T>` | Swift array | Native |

Generated records and enums come with `Equatable`, `Hashable`, and `Sendable`
for free, so they drop straight into SwiftUI `List` / `ForEach` /
`navigationDestination(for:)`.

### Error mapping

Fallible facade functions return `Result<T, PergamonError>`, where
`PergamonError` is a **single, flat** error enum owned by the facade and
annotated `#[derive(uniffi::Error)]`. UniFFI maps this to Swift's `throws`, so
Apple code uses ordinary `do / try / catch`.

- **One flattened enum, not per-crate mirrors.** The facade wraps
  `pergamon-core` and (once the `Library` handle exists) `pergamon-storage`,
  `pergamon-feed`, and `pergamon-extract`. Rather than exporting each crate's
  `thiserror` enum, the facade collapses them into one `PergamonError` with a
  small, stable set of variants that describe categories the app can act on —
  for example `NotFound`, `InvalidInput`, `Storage`, `Network`, and `Internal`.
- **Boundary conversion.** `From<CoreError>`, `From<StorageError>`, etc., live
  in the facade and map each internal variant to the appropriate `PergamonError`
  category. Internal error detail that the app cannot act on is folded into a
  human-readable message rather than widening the FFI ABI. For example
  `StorageError::NotFound { entity, id }` → `PergamonError::NotFound` carrying a
  message; `StorageError::Sqlite(..)` and `CoreError::*` parse failures →
  `PergamonError::Internal` / `InvalidInput`.
- **Each variant carries a message.** Variants expose a human-readable
  `message: String` and, where it helps the app branch, a stable
  machine-readable discriminant (the variant itself). Swift can show the message
  and switch on the case. This keeps error handling ergonomic without leaking a
  brittle internal error taxonomy.
- **No panics across the boundary.** Facade functions must not `panic!`,
  `unwrap`, or `expect` on caller-controlled input; a panic unwinding into
  generated FFI glue is undefined behavior. Fallible operations return `Result`;
  programmer invariants that genuinely cannot fail are asserted internally, never
  on the Swift-reachable path.

`pergamon-core` keeps using `thiserror`; the facade keeps using `thiserror`
plus the `uniffi::Error` derive. `anyhow` remains confined to binary crates per
existing convention and never crosses the FFI.

### Threading and async model

Core logic is pure and fast (state transitions, FSRS scheduling, reading-time
estimation, canonicalization), and on-device SQLite access behind the `Library`
handle is local and quick. The default model is therefore **synchronous,
blocking calls**, not `async`.

- **Records are `Sendable`; handles are `Send + Sync`.** Value records cross
  threads freely. The `Library` handle guards its interior state (the SQLite
  connection and any caches) so its methods are safe to call from any thread.
- **Blocking calls, invoked off the main actor.** Facade functions block the
  calling thread until they return. SwiftUI code calls them from a background
  context (e.g. inside a `Task`/actor, off `@MainActor`) and publishes results
  back to the UI, exactly as it would for any synchronous I/O. UniFFI does not
  hop threads for blocking calls, so the app is responsible for not calling them
  on the main thread during large operations.
- **`async fn` reserved for genuinely asynchronous work.** UniFFI's `async fn`
  support (which bridges to Swift `async`) is reserved for operations that are
  actually asynchronous — network fetches and sync — which land behind the
  facade in later Phase 6 / sync work. Pure and local-DB operations stay
  blocking to avoid the runtime and complexity cost of async for work that never
  waits. Per ADR-007, HTTP itself still lives outside the core; any `async`
  facade method wraps an orchestration layer, not networking inside
  `pergamon-core`.

### Binding ABI and versioning policy

The Swift bindings are a **generated build artifact**, not hand-maintained
source, regenerated from the Rust facade on every build.

- **Generator pinned to runtime.** The vendored `uniffi-bindgen` bin guarantees
  the generated Swift always matches the linked `uniffi` runtime. Bumping
  `uniffi` regenerates bindings and rebuilds the `PergamonFFI.xcframework` in the
  same change.
- **The facade surface is the versioned ABI.** The exported records, enums,
  function signatures, and handle methods *are* the contract. The
  `pergamon-uniffi` crate version (workspace-versioned) tracks it.
- **Change classification.** Adding a record field with a default-able Swift
  representation, adding an enum variant the app can ignore, or adding a new
  function/handle method is an **additive (minor)** change. Renaming or removing
  a field/variant/function, changing a signature or a type mapping, or changing
  error variants is a **breaking** change that requires a coordinated app update.
- **No separate hand-written ABI shim.** Because bindings are regenerated, there
  is no `.udl` or C header to version independently. The XCFramework is rebuilt
  and re-embedded per release (#113, #120); it is git-ignored and regenerated on
  demand rather than committed.

## Consequences

### Positive

- A single narrow facade keeps ergonomics and binary size healthy (spike:
  ~0.5 MB stripped) and shields Apple clients from internal crate churn.
- Proc-macro mode keeps the contract in Rust with no `.udl` to drift, and the
  vendored generator removes version-skew failures.
- One flat `PergamonError` gives Swift idiomatic `throws` with actionable
  categories, without exporting a brittle multi-crate error taxonomy.
- `pergamon-core` stays zero-I/O and `unsafe`-`forbid`; only the thin facade
  relaxes the lint, preserving the ADR-001 / ADR-007 boundaries.
- Blocking-by-default avoids async runtime overhead for pure and local-DB work,
  while `async fn` remains available for real networking/sync later.
- Regenerated bindings with a pinned generator make the ABI reproducible and
  keep versioning tied to the source of truth.

### Negative

- The facade must hand-write `From` conversions and mirrored enums/records; each
  new app feature that needs core data adds a small mapping burden.
- Flattening errors loses fine-grained internal variants at the boundary; some
  detail survives only as a message string, so deep programmatic branching on
  internal causes is not possible from Swift by design.
- Blocking calls put the burden on the app to stay off the main thread for large
  operations; a careless main-thread call can jank the UI.
- Mirrored enums can drift from their `pergamon-core` counterparts if a new core
  variant is not reflected in the facade; a compile-time exhaustiveness check on
  the `From` conversion is needed to catch this.
- Breaking ABI changes require rebuilding and re-shipping the XCFramework in
  lockstep with the app.

## Rejected Alternatives

- **UDL-based UniFFI (`.udl` interface file):** rejected because it duplicates
  the type definitions that already live in Rust and must be kept in sync by
  hand. Proc-macro mode keeps the contract next to the types and was validated by
  the spike.
- **Exposing internal crate types directly to Swift:** rejected because it
  couples the Apple ABI to internal refactors, widens the surface (hurting binary
  size and ergonomics), and leaks storage/connection machinery the app should
  never see. The facade exports product-shaped types only.
- **Per-crate mirrored FFI error enums:** rejected because Swift would face a
  sprawling, unstable error taxonomy that changes whenever an internal crate
  adds a variant. A single flat `PergamonError` with actionable categories plus a
  message is more ergonomic and more stable.
- **Making the whole surface `async`:** rejected because core logic and local
  SQLite access do not wait on anything; wrapping them in `async` adds runtime
  and reasoning cost for no benefit. `async fn` is reserved for genuinely
  asynchronous networking and sync.
- **Hand-written C FFI (or a committed C header / bindings):** rejected because
  it reintroduces the `unsafe` glue and version-skew problems UniFFI exists to
  eliminate. Bindings are a regenerated artifact pinned to the runtime.
