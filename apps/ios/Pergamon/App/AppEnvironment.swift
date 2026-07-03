import Foundation
import PergamonKit

extension Notification.Name {
    /// Posted whenever review state changes (a highlight is captured or deleted,
    /// or a card is graded) so the Review tab's due-count badge can refresh
    /// without polling. Kept app-level so any surface can raise it.
    static let pergamonReviewStateChanged = Notification.Name("pergamonReviewStateChanged")

    /// Posted after a bulk change to the whole library (a backup restore) so
    /// every list surface reloads from the core instead of showing stale rows.
    static let pergamonLibraryDidChange = Notification.Name("pergamonLibraryDidChange")
}

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
    /// The stateful entry point into `pergamon-core`. Backed by the on-device
    /// SQLite store at `storage.databaseURL` (#118), or — if that database
    /// cannot be opened — by an in-memory seeded corpus so the app still
    /// launches. See `usingPersistentStore`.
    let library: Library

    /// Where the on-device store lives (ADR-020). Resolved and prepared on
    /// launch and handed to `Library.open` (#118).
    let storage: StorageLocation

    /// The resolved `pergamon-core` version, surfaced in the UI as provenance.
    let coreVersion: String

    /// `true` when `library` is backed by the persistent SQLite database;
    /// `false` when the app fell back to the in-memory seed (e.g. a corrupt or
    /// unwritable database). Surfaced in the backup/settings UI.
    let usingPersistentStore: Bool

    /// The share-extension staging drop folder (ADR-021), or `nil` when the App
    /// Group is unavailable (a bare Simulator with no signing team). When `nil`,
    /// share-sheet capture is inert and finalization is a no-op.
    let staging: StagingInbox?

    init() {
        // 1. Resolve + prepare the on-device storage container (ADR-020). This
        //    creates the App Group directory tree and excludes blobs from
        //    backup.
        let storage = StorageLocation.resolve()
        self.storage = storage

        // 2. Open the SQLite-backed library at `storage.databaseURL` (#118),
        //    seeding the demo corpus on first launch. If opening fails (a
        //    corrupt database, an unwritable container), fall back to the
        //    in-memory seed so the app always launches.
        do {
            self.library = try Library.open(path: storage.databaseURL.path)
            self.usingPersistentStore = true
        } catch {
            print("[bootstrap] failed to open SQLite library at \(storage.databaseURL.path): \(error)")
            print("[bootstrap] falling back to in-memory seeded corpus")
            self.library = Library()
            self.usingPersistentStore = false
        }
        self.coreVersion = libraryVersion()

        // 3. Resolve the shared staging drop folder the share extension writes
        //    to (ADR-021). Absent on a Simulator with no provisioned App Group.
        self.staging = StagingInbox.shared()

        logBootstrap()
    }

    /// Drains any captures the share extension has staged into the library, then
    /// notifies list surfaces to reload if anything changed.
    ///
    /// Runs the ingestion off the main actor (core reads/writes are synchronous
    /// and thread-safe per ADR-019) and posts ``Notification/Name/pergamonLibraryDidChange``
    /// back on the main actor. Called on launch and on every foreground, so a
    /// page saved from the share sheet appears the next time the app is seen.
    func finalizePendingCaptures() {
        guard let staging else { return }
        let library = self.library
        Task.detached(priority: .utility) {
            let result = StagingFinalizer(library: library, inbox: staging).drain()
            guard result.didChange else { return }
            await MainActor.run {
                NotificationCenter.default.post(name: .pergamonLibraryDidChange, object: nil)
                NotificationCenter.default.post(name: .pergamonReviewStateChanged, object: nil)
            }
        }
    }

    private func logBootstrap() {
        let backing = storage.usingAppGroup ? "App Group" : "app container (fallback)"
        let store = usingPersistentStore ? "SQLite" : "in-memory seed (fallback)"
        print("[bootstrap] pergamon-core \(coreVersion)")
        print("[bootstrap] library: \(store)")
        print("[bootstrap] storage: \(backing) → \(storage.databaseURL.path)")
        print("[bootstrap] blobs (backup-excluded): \(storage.blobsURL.path)")
        if let staging {
            print("[bootstrap] share staging: \(staging.directory.path)")
        } else {
            print("[bootstrap] share staging: unavailable (App Group not provisioned)")
        }
    }
}
