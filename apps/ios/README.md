# pergamon iOS app + PergamonKit

The Rust `pergamon-core` library is consumed on Apple platforms through
**PergamonKit** — an idiomatic Swift wrapper over the UniFFI-generated bindings,
packaged with the Rust core as an `XCFramework`. The conventions are fixed by
[ADR-019](../../docs/adr/019-uniffi-boundary-and-error-mapping.md); the reference
implementation (flat `PergamonError`, the stateful `Library` handle, and the
Swift package) landed with issue #113.

`apps/ios/PergamonSpike` is a minimal SwiftUI app that **lists** and **opens**
items served entirely by the Rust core via the `Library` handle. It consumes the
XCFramework through the PergamonKit package with **no hand-written FFI glue**.
The corpus is an in-memory seed today; the on-device SQLite store lands with the
offline-database work (#118 / ADR-020), behind the same `Library` surface.

## Layout

| Path | Committed? | What |
|------|-----------|------|
| `PergamonKit/Package.swift` | yes | SwiftPM package: wrapper + binary/bindings targets + tests |
| `PergamonKit/Sources/PergamonKit/*.swift` | yes | idiomatic wrapper (re-exports, `Identifiable`, `Date`, labels) |
| `PergamonKit/Tests/PergamonKitTests/*.swift` | yes | XCTest suite (`swift test`) |
| `PergamonSpike/*.swift` | yes | SwiftUI sources (app, inbox list, detail view) |
| `HostSmoke/main.swift` | yes | host-side smoke test (`scripts/smoke-macos.sh`) |
| `project.yml` | yes | xcodegen spec for the app target |
| `PergamonKit/Sources/PergamonBindings/*.swift` | no (generated) | UniFFI Swift bindings |
| `PergamonKit/Frameworks/PergamonFFI.xcframework` | no (generated) | Rust static-lib XCFramework (iOS device + simulator + macOS) |
| `PergamonSpike.xcodeproj` | no (generated) | produced by `xcodegen` |

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
xcodebuild -project PergamonSpike.xcodeproj -scheme PergamonSpike \
  -destination 'platform=iOS Simulator,name=iPhone 16' build

# 3. (optional) install + launch in a booted simulator.
xcrun simctl boot 'iPhone 16' || true
APP=$(find ~/Library/Developer/Xcode/DerivedData -name PergamonSpike.app \
  -path '*Debug-iphonesimulator*' | head -1)
xcrun simctl install booted "$APP"
xcrun simctl launch booted dev.pergamon.spike
```

## Fast inner loop (no Xcode, no package)

To validate the raw binding contract directly against the generated bindings:

```sh
./scripts/smoke-macos.sh
```

This links the macOS build of the facade and runs `HostSmoke/main.swift` against
the generated bindings, exercising the `Library` handle and the throwing
`item(id:)` error path.
