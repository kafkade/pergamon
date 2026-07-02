import SwiftUI

/// The production pergamon iPhone app.
///
/// Content is served entirely by the Rust `pergamon-core` library through the
/// PergamonKit wrapper (UniFFI bindings) — the Swift side only renders it, with
/// no hand-written FFI glue (ADR-019). The app owns a single `AppEnvironment`
/// (the composition root / DI container) and injects it into the view tree.
@main
struct PergamonApp: App {
    /// The single DI container for the app's lifetime. `@StateObject` ensures it
    /// is created once and survives view updates; its `init` runs the launch
    /// bootstrap (storage container + core handle).
    @StateObject private var environment = AppEnvironment()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(environment)
        }
    }
}
