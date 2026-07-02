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

## App architecture

- **`AppEnvironment`** is the composition root / DI container. It is created once
  at launch, holds the single `Library` handle, and is injected into the view
  tree with `.environmentObject`. Every screen reads `library` from it rather
  than constructing its own.
- **`StorageLocation`** resolves *where* the on-device store lives per ADR-020:
  a shared **App Group** container (`group.dev.pergamon`) with
  `pergamon.db` + a backup-excluded `blobs/` tree. It runs on launch and is the
  seam the SQLite-backed `Library` (#118) slots into. If the App Group is not
  provisioned (e.g. a bare Simulator with no signing team) it falls back to the
  app container, so the app always launches.
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

## Layout

| Path | Committed? | What |
|------|-----------|------|
| `PergamonKit/Package.swift` | yes | SwiftPM package: wrapper + binary/bindings targets + tests |
| `PergamonKit/Sources/PergamonKit/*.swift` | yes | idiomatic wrapper (re-exports, `Identifiable`, `Date`, labels) |
| `PergamonKit/Tests/PergamonKitTests/*.swift` | yes | XCTest suite (`swift test`) |
| `Pergamon/App/*.swift` | yes | app entry point, DI container, ADR-020 storage seam |
| `Pergamon/Root/RootView.swift` | yes | tab/navigation shell |
| `Pergamon/Features/**/*.swift` | yes | Inbox, Saved, Search, Review, Detail screens |
| `Pergamon/Components/*.swift` | yes | shared list row |
| `Pergamon/Presentation/*.swift` | yes | SwiftUI styling (SF Symbols, tints) |
| `Pergamon/Pergamon.entitlements` | yes | App Group `group.dev.pergamon` |
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
