// swift-tools-version: 5.9
import PackageDescription

// PergamonKit — the idiomatic Swift wrapper over the UniFFI-generated bindings
// for `pergamon-core` (ADR-019). The app imports only `PergamonKit` and never
// touches the generated FFI symbols directly.
//
// The binary artifacts this package depends on are produced by
// `scripts/build-ios.sh` and are git-ignored:
//   - Frameworks/PergamonFFI.xcframework            (Rust static-lib slices)
//   - Sources/PergamonBindings/pergamon_uniffi.swift (generated bindings)
// Run `./scripts/build-ios.sh` before `swift build` / `swift test`.
let package = Package(
    name: "PergamonKit",
    platforms: [
        .iOS(.v17),
        // macOS is supported so `swift test` runs natively on the host (a fast
        // inner loop with no Simulator); the XCFramework ships a macOS slice.
        .macOS(.v14),
    ],
    products: [
        .library(name: "PergamonKit", targets: ["PergamonKit"]),
    ],
    targets: [
        // The Rust core, compiled to static libraries and packaged as an
        // XCFramework (iOS device + simulator + macOS host slices).
        .binaryTarget(
            name: "PergamonFFI",
            path: "Frameworks/PergamonFFI.xcframework"
        ),
        // The UniFFI-generated Swift bindings. Pure generated code — do not edit.
        .target(
            name: "PergamonBindings",
            dependencies: ["PergamonFFI"]
        ),
        // The idiomatic wrapper the app consumes.
        .target(
            name: "PergamonKit",
            dependencies: ["PergamonBindings"]
        ),
        .testTarget(
            name: "PergamonKitTests",
            dependencies: ["PergamonKit"]
        ),
    ]
)
