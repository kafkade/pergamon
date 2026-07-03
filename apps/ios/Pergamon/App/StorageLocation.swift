import Foundation

/// Resolves *where* the on-device library lives, per **ADR-020** (mobile storage
/// ownership and cache policy).
///
/// The canonical store is one SQLite database plus a content-addressed blob tree
/// under a shared **App Group** container, so the main app and the (future)
/// share extension (#119) mount the *same* library:
///
/// ```text
/// <AppGroup>/Library/Application Support/pergamon/
/// ├── pergamon.db      # SQLite: metadata, extracted text, FTS5, annotations, cards
/// └── blobs/           # content-addressed raw assets (excluded from backup)
/// ```
///
/// ## Status
///
/// This type is the **bootstrap seam** for the offline database (#118), now
/// wired: `AppEnvironment` opens `Library.open(databaseURL.path)` against the
/// resolved container, the blob tree is excluded from iCloud backup (ADR-020
/// §4), and the path is logged on launch. If the database cannot be opened the
/// app falls back to an in-memory seed so it always launches.
///
/// ## Simulator fallback
///
/// `containerURL(forSecurityApplicationGroupIdentifier:)` returns `nil` when the
/// App Group entitlement is not provisioned (common on a bare simulator with no
/// signing team). Rather than crash — which would break "the app launches on a
/// simulator" — we fall back to the app's own Application Support directory.
/// The layout is identical; only the parent container differs.
struct StorageLocation {
    /// The App Group identifier both the app and the share extension share.
    /// Must match `com.apple.security.application-groups` in `Pergamon.entitlements`.
    /// Delegates to ``AppGroup/identifier`` so there is one source of truth.
    static let appGroupIdentifier = AppGroup.identifier

    /// Directory that holds `pergamon.db` and `blobs/`.
    let root: URL
    /// Canonical SQLite database URL (the #118 hand-off point).
    let databaseURL: URL
    /// Content-addressed blob directory (excluded from backup).
    let blobsURL: URL
    /// `true` when backed by the shared App Group container; `false` when we fell
    /// back to the app container (App Group not provisioned).
    let usingAppGroup: Bool

    private static let subpath = "Library/Application Support/pergamon"

    /// Resolves the storage location and ensures its directories exist.
    ///
    /// Prefers the shared App Group container and falls back to the app's own
    /// Application Support directory when the App Group is unavailable. Creating
    /// the directories and excluding `blobs/` from backup are best-effort: any
    /// failure is logged but never fatal, so the app always launches.
    static func resolve(
        fileManager: FileManager = .default
    ) -> StorageLocation {
        let (root, usingAppGroup) = rootURL(fileManager: fileManager)
        let blobs = root.appendingPathComponent("blobs", isDirectory: true)

        create(directory: root, fileManager: fileManager)
        create(directory: blobs, fileManager: fileManager)
        excludeFromBackup(blobs)

        return StorageLocation(
            root: root,
            databaseURL: root.appendingPathComponent("pergamon.db", isDirectory: false),
            blobsURL: blobs,
            usingAppGroup: usingAppGroup
        )
    }

    /// The `.../pergamon` directory that holds `pergamon.db` and `blobs/`, plus
    /// whether it lives in the shared App Group container.
    private static func rootURL(fileManager: FileManager) -> (URL, Bool) {
        if let group = fileManager.containerURL(
            forSecurityApplicationGroupIdentifier: appGroupIdentifier
        ) {
            // Mirror the desktop layout inside the App Group container so the
            // core's storage code stays platform-agnostic.
            return (group.appendingPathComponent(subpath, isDirectory: true), true)
        }
        // Fallback: the app's own Application Support directory (already the
        // `Library/Application Support` location), so a bare Simulator with no
        // provisioned App Group still launches.
        let appSupport = (try? fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )) ?? fileManager.temporaryDirectory
        return (appSupport.appendingPathComponent("pergamon", isDirectory: true), false)
    }

    private static func create(directory: URL, fileManager: FileManager) {
        do {
            try fileManager.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
        } catch {
            print("[storage] failed to create \(directory.path): \(error)")
        }
    }

    /// Excludes the blob cache from iCloud / device backup (ADR-020 §4): it is
    /// large and reconstructable, so backing it up wastes the user's quota.
    private static func excludeFromBackup(_ url: URL) {
        var url = url
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        do {
            try url.setResourceValues(values)
        } catch {
            print("[storage] failed to exclude \(url.path) from backup: \(error)")
        }
    }
}
