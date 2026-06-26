import SwiftUI

/// Minimal SwiftUI app that lists and opens items sourced entirely from the
/// Rust `pergamon-core` library through the UniFFI-generated bindings.
///
/// Spike deliverable for issue #29. Every piece of content shown here is built
/// in Rust (`sampleItems()` / `getItem(id:)`) — the Swift side only renders it.
@main
struct PergamonSpikeApp: App {
    var body: some Scene {
        WindowGroup {
            InboxView()
        }
    }
}
