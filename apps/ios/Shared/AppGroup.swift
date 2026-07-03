import Foundation

/// The shared **App Group** both the main app and the share extension mount, per
/// **ADR-020** (mobile storage ownership) and **ADR-021** (share-extension
/// ingestion contract).
///
/// This is the single source of truth for the group identifier and the on-disk
/// layout that spans the two processes. It is compiled into *both* the app and
/// the extension targets, so neither hard-codes the identifier independently.
///
/// ```text
/// <AppGroup>/Library/Application Support/pergamon/
/// ├── pergamon.db          # canonical SQLite store (app-owned)
/// ├── blobs/               # content-addressed raw assets (backup-excluded)
/// └── staging/inbox/       # share-extension drop folder (ADR-021)
///     └── <capture_id>.json
/// ```
///
/// The extension only ever writes to `staging/inbox/`; it never opens the
/// database. The app drains `staging/inbox/` and owns everything else.
enum AppGroup {
    /// The App Group identifier shared across the app and extension. Must match
    /// `com.apple.security.application-groups` in both targets' entitlements and
    /// `StorageLocation.appGroupIdentifier`.
    static let identifier = "group.dev.pergamon"

    /// Root of the pergamon library inside the shared container, mirroring the
    /// desktop layout so the Rust storage code stays platform-agnostic.
    private static let librarySubpath = "Library/Application Support/pergamon"

    /// The share-extension drop folder, relative to `librarySubpath`.
    private static let stagingSubpath = "staging/inbox"

    /// The shared container root, or `nil` when the App Group entitlement is not
    /// provisioned (a bare Simulator with no signing team, on which sharing
    /// between the two processes is not possible).
    static func containerURL(fileManager: FileManager = .default) -> URL? {
        fileManager.containerURL(forSecurityApplicationGroupIdentifier: identifier)
    }

    /// The `.../staging/inbox` drop folder in the shared container, or `nil` when
    /// the App Group is unavailable. Callers create it on demand.
    static func stagingInboxURL(fileManager: FileManager = .default) -> URL? {
        containerURL(fileManager: fileManager)?
            .appendingPathComponent(librarySubpath, isDirectory: true)
            .appendingPathComponent(stagingSubpath, isDirectory: true)
    }
}
