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

    /// Drives share-extension finalization: we drain the staging drop folder
    /// whenever the app becomes active (ADR-021's scan-on-open baseline).
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(environment)
                .task {
                    // Launch drain: pick up anything staged while we were gone.
                    environment.finalizePendingCaptures()
                }
        }
        .onChange(of: scenePhase) { _, phase in
            // Foreground drain: a capture shared while the app was backgrounded
            // lands the next time the user opens it.
            if phase == .active {
                environment.finalizePendingCaptures()
            }
        }
    }
}
