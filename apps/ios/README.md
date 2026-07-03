# pergamon iOS app + PergamonKit

The Rust `pergamon-core` library is consumed on Apple platforms through
**PergamonKit** — an idiomatic Swift wrapper over the UniFFI-generated bindings,
packaged with the Rust core as an `XCFramework`. The conventions are fixed by
[ADR-019](../../docs/adr/019-uniffi-boundary-and-error-mapping.md); the reference
implementation (flat `PergamonError`, the stateful `Library` handle, and the
Swift package) landed with issue #113.

`apps/ios/Pergamon` is the production SwiftUI iPhone app: a tab/navigation shell
(**Inbox**, **Saved**, **Search**, **Review**) over content served entirely by
the Rust core via the `Library` handle. It consumes the XCFramework through the
PergamonKit package with **no hand-written FFI glue**. The corpus is an in-memory
seed today; the on-device SQLite store lands with the offline-database work
(#118 / [ADR-020](../../docs/adr/020-mobile-storage-ownership-and-cache-policy.md)),
behind the same `Library` surface.

`apps/ios/ShareExtension` is the **share-sheet capture extension**: it accepts a
URL and/or selected text from any app's share sheet and stages it for the main
app to ingest, per
[ADR-021](../../docs/adr/021-share-extension-ingestion-contract.md). The
extension does **no** networking, extraction, or database access — it writes one
atomic JSON drop file per capture into the shared App Group and dismisses
immediately, so sharing stays fast and works offline. The main app finalizes
staged captures on its next launch/foreground.

## App architecture

- **`AppEnvironment`** is the composition root / DI container. It is created once
  at launch, holds the single `Library` handle, and is injected into the view
  tree with `.environmentObject`. Every screen reads `library` from it rather
  than constructing its own.
- **`StorageLocation`** resolves *where* the on-device store lives per ADR-020:
  a shared **App Group** container (`group.dev.pergamon`) with
  `pergamon.db` + a backup-excluded `blobs/` tree, plus the
  `staging/inbox/` drop folder the share extension writes to (ADR-021). It runs
  on launch and is the seam the SQLite-backed `Library` (#118) slots into. If the
  App Group is not provisioned (e.g. a bare Simulator with no signing team) it
  falls back to the app container, so the app always launches.
- **`RootView`** is the `TabView` shell; each tab owns its own
  `NavigationStack`. `DetailView` is the shared **reader**: it re-fetches by id
  via `library.item(id:)` (exercising the throwing FFI path) and renders the
  normalized extracted content (`contentText`) as a readable article. Because
  the content is served entirely from the local core, the reader works offline.
- **Inbox triage** (`InboxView`) filters by status, feed/source
  (`library.sources()`), and read/unread state, and exposes per-item swipe
  actions — mark read/unread, save for later, archive — that call the `Library`
  triage mutations and refresh in place. The reader offers the same actions from
  its toolbar. Mutations apply to the in-memory corpus today; the SQLite store
  (#118) persists them across launches behind the same surface.
- **Search** (`SearchView`) runs `library.searchFiltered(query:facets:)` with
  faceted filters — content type, status, tag, source, and a published-within
  date preset — mirroring the CLI/web facet set. Active facets show as removable
  chips.
- **Bookmarks & organization** (`SavedView`) folds collection and tag browsing
  into the Saved tab via a scope picker (All / Status / Collections / Tags),
  including a nested collection tree with per-collection counts. The reader's
  **Organize** sheet (`OrganizeSheet`) assigns and removes tags and collection
  memberships — assigning existing entries or creating new ones inline — through
  the `Library` organization mutations.

## Share extension (capture)

`ShareExtension` implements ADR-021's **stage-then-finalize** split so the share
sheet is fast and offline-safe, and all ingestion logic lives in one place (the
Rust core), never duplicated in the extension.

- **`ShareViewController`** is the extension entry point. It reads the shared
  `NSItemProvider`s — a URL, plain text, or both — classifies the capture
  (`url`, `url_with_selection`, or `text`), and writes a single `StagedCapture`
  to the App Group drop folder, then completes immediately. It never touches the
  network or the database.
- **`Shared/`** holds the code compiled into *both* the app and the extension:
  `AppGroup` (the group id + on-disk layout), `StagedCapture` (the on-disk JSON
  contract, independent of the UniFFI `ShareCapture`), and `StagingInbox` (the
  filesystem contract — atomic `.json.tmp`→rename writes, oldest-first draining,
  post-commit deletes).
- **`StagingFinalizer`** is the app-side drainer. It reads pending captures
  oldest-first, maps each `StagedCapture` onto the core's `ShareCapture` FFI
  record, calls `Library.ingestShareCapture` (canonicalize → dedupe →
  create/enrich bookmark → attach highlight), and deletes the file **only after**
  the write commits. A crash between commit and delete simply reprocesses the
  survivor; the core's dedupe (canonical URL, or `capture_id` for text) makes
  that converge rather than duplicate. `AppEnvironment.finalizePendingCaptures()`
  runs it on launch (`.task`) and on every foreground (`scenePhase == .active`),
  then posts library/review change notifications so open surfaces reload.

Captured URLs land as **bookmarks in the inbox**; deferred fetch + readability
extraction (upgrading a bookmark to a full article) is intentionally out of scope
for #119 and happens later behind the same `Library` surface.

## Layout

| Path | Committed? | What |
|------|-----------|------|
| `PergamonKit/Package.swift` | yes | SwiftPM package: wrapper + binary/bindings targets + tests |
| `PergamonKit/Sources/PergamonKit/*.swift` | yes | idiomatic wrapper (re-exports, `Identifiable`, `Date`, labels) |
| `PergamonKit/Tests/PergamonKitTests/*.swift` | yes | XCTest suite (`swift test`) |
| `Pergamon/App/*.swift` | yes | app entry point, DI container, ADR-020 storage seam, ADR-021 finalizer |
| `Pergamon/Root/RootView.swift` | yes | tab/navigation shell |
| `Pergamon/Features/**/*.swift` | yes | Inbox, Saved, Search, Review, Detail, Organize screens |
| `Pergamon/Components/*.swift` | yes | shared list row, wrapping chip layout |
| `Pergamon/Presentation/*.swift` | yes | SwiftUI styling (SF Symbols, tints) |
| `Pergamon/Pergamon.entitlements` | yes | App Group `group.dev.pergamon` |
| `Shared/*.swift` | yes | app+extension shared code: App Group, staging contract, inbox |
| `ShareExtension/ShareViewController.swift` | yes | share-sheet capture → staged JSON |
| `ShareExtension/Info.plist` | yes | `NSExtension` share activation rules |
| `ShareExtension/ShareExtension.entitlements` | yes | App Group `group.dev.pergamon` |
| `HostSmoke/main.swift` | yes | host-side smoke test (`scripts/smoke-macos.sh`) |
| `project.yml` | yes | xcodegen spec for the app target |
| `PergamonKit/Sources/PergamonBindings/*.swift` | no (generated) | UniFFI Swift bindings |
| `PergamonKit/Frameworks/PergamonFFI.xcframework` | no (generated) | Rust static-lib XCFramework (iOS device + simulator + macOS) |
| `Pergamon.xcodeproj` | no (generated) | produced by `xcodegen` |

The generated artifacts are git-ignored and rebuilt on demand by
`scripts/build-ios.sh`.

## Prerequisites

- Xcode (with iOS SDK + simulators)
- Rust toolchain (`rustup`)
- [`xcodegen`](https://github.com/yonaskolb/XcodeGen): `brew install xcodegen`

## Build the XCFramework + bindings

Everything downstream depends on this step, which builds the Rust core for iOS
device, iOS simulator, and the macOS host, generates the Swift bindings, and
assembles `PergamonKit/Frameworks/PergamonFFI.xcframework`:

```sh
./scripts/build-ios.sh
```

## Run the Swift unit tests (fast, no Simulator)

PergamonKit's tests run natively on the macOS host via the XCFramework's macOS
slice:

```sh
cd apps/ios/PergamonKit && swift test
```

## Build & run the app

From the repo root, after `./scripts/build-ios.sh`:

```sh
# 1. Generate the Xcode project.
cd apps/ios && xcodegen generate

# 2. Build for a simulator.
xcodebuild -project Pergamon.xcodeproj -scheme Pergamon \
  -destination 'platform=iOS Simulator,name=iPhone 16' build

# 3. (optional) install + launch in a booted simulator.
xcrun simctl boot 'iPhone 16' || true
APP=$(find ~/Library/Developer/Xcode/DerivedData -name Pergamon.app \
  -path '*Debug-iphonesimulator*' | head -1)
xcrun simctl install booted "$APP"
xcrun simctl launch booted dev.pergamon.app
```

The app's App Group entitlement is applied ad-hoc on the Simulator, so no
signing team is required for simulator builds. **Device** builds need a signing
team with `group.dev.pergamon` registered on the provisioning profile.

## Fast inner loop (no Xcode, no package)

To validate the raw binding contract directly against the generated bindings:

```sh
./scripts/smoke-macos.sh
```

This links the macOS build of the facade and runs `HostSmoke/main.swift` against
the generated bindings, exercising the `Library` handle and the throwing
`item(id:)` error path.
