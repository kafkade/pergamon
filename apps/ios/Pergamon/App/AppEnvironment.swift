import Foundation
import PergamonKit

/// The app's dependency-injection container and single owner of the Rust core
/// handle.
///
/// One `AppEnvironment` is created at launch, injected into the view tree with
/// `.environmentObject`, and read by every screen — so the whole app shares a
/// single `Library` rather than each view constructing its own. This is the
/// composition root: when the data layer grows (a SQLite-backed `Library` from
/// #118, background services, settings), it is wired here and the views keep
/// reading `library` unchanged.
///
/// `Library` is a reference-type UniFFI object handle whose reads are
/// synchronous and thread-safe (ADR-019), so it is safe to hold here and call
/// from view models.
@MainActor
final class AppEnvironment: ObservableObject {
    /// The stateful entry point into `pergamon-core`. Backed by an in-memory
    /// seeded corpus today; the on-device SQLite store lands with #118 (see
    /// `StorageLocation`).
    let library: Library

    /// Where the on-device store lives (ADR-020). Resolved and prepared on
    /// launch; not yet handed to `Library` (that is the #118 seam).
    let storage: StorageLocation

    /// The resolved `pergamon-core` version, surfaced in the UI as provenance.
    let coreVersion: String

    init() {
        // 1. Resolve + prepare the on-device storage container (ADR-020). This
        //    creates the App Group directory tree and excludes blobs from
        //    backup, even though the in-memory Library does not read it yet.
        let storage = StorageLocation.resolve()
        self.storage = storage

        // 2. Open the core. TODO(#118): open the SQLite-backed library at
        //    `storage.databaseURL` instead of the in-memory seed.
        self.library = Library()
        self.coreVersion = libraryVersion()

        logBootstrap()
    }

    private func logBootstrap() {
        let backing = storage.usingAppGroup ? "App Group" : "app container (fallback)"
        print("[bootstrap] pergamon-core \(coreVersion)")
        print("[bootstrap] storage: \(backing) → \(storage.databaseURL.path)")
        print("[bootstrap] blobs (backup-excluded): \(storage.blobsURL.path)")
    }
}
