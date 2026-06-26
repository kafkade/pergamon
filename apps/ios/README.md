# pergamon iOS sample app (spike #29)

A minimal SwiftUI app that **lists** and **opens** content items sourced entirely
from the Rust `pergamon-core` library through UniFFI-generated Swift bindings.

This is a **proof of concept** for issue #29 — it validates that the zero-I/O Rust
core is consumable from SwiftUI. It serves an in-memory seeded sample corpus
(there is no SQLite binding on the Apple side yet). See
[`docs/spikes/uniffi-ios-findings.md`](../../docs/spikes/uniffi-ios-findings.md)
for the full write-up.

## Layout

| Path | Committed? | What |
|------|-----------|------|
| `project.yml` | ✅ | xcodegen spec for the app target |
| `PergamonSpike/*.swift` | ✅ | SwiftUI sources (app, inbox list, detail view) |
| `HostSmoke/main.swift` | ✅ | host-side smoke test (`scripts/smoke-macos.sh`) |
| `PergamonSpike/Generated/` | ❌ (generated) | UniFFI Swift bindings |
| `Frameworks/PergamonFFI.xcframework` | ❌ (generated) | Rust static-lib XCFramework |
| `PergamonSpike.xcodeproj` | ❌ (generated) | produced by `xcodegen` |

The generated artifacts are git-ignored and rebuilt on demand.

## Prerequisites

- Xcode (with iOS SDK + simulators)
- Rust toolchain (`rustup`)
- [`xcodegen`](https://github.com/yonaskolb/XcodeGen): `brew install xcodegen`

## Build & run

From the repo root:

```sh
# 1. Build the Rust core for iOS, generate bindings, assemble the XCFramework.
./scripts/build-ios.sh

# 2. Generate the Xcode project.
cd apps/ios && xcodegen generate

# 3. Build for a simulator.
xcodebuild -project PergamonSpike.xcodeproj -scheme PergamonSpike \
  -destination 'platform=iOS Simulator,name=iPhone 17' build

# 4. (optional) install + launch in a booted simulator.
xcrun simctl boot 'iPhone 17' || true
APP=$(find build -name PergamonSpike.app -path '*Debug-iphonesimulator*' | head -1)
xcrun simctl install booted "$APP"
xcrun simctl launch booted dev.pergamon.spike
```

## Fast inner loop (no Xcode)

To validate the binding contract without building the app:

```sh
./scripts/smoke-macos.sh
```

This links the macOS build of the facade and runs `HostSmoke/main.swift` against
the generated bindings.
