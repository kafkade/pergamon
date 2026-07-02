import SwiftUI
import PergamonKit

/// Minimal SwiftUI app that lists and opens items sourced entirely from the
/// Rust `pergamon-core` library through the PergamonKit wrapper (UniFFI
/// bindings). Every piece of content shown here is built in Rust and driven via
/// the `Library` handle — the Swift side only renders it, with no hand-written
/// FFI glue.
@main
struct PergamonSpikeApp: App {
    var body: some Scene {
        WindowGroup {
            InboxView()
        }
    }
}
