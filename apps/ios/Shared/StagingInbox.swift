import Foundation

/// The append-only **drop folder** the share extension writes to and the main
/// app drains, per **ADR-021**.
///
/// One file per capture, published atomically (write to a `.json.tmp` sibling,
/// then rename to `.json`), so a reader never sees a partial write and a crash
/// mid-write leaves only an ignorable `.tmp`. The extension only ever *creates*
/// files here; only the app deletes them, and only after the capture is durably
/// ingested.
///
/// This type owns just the filesystem contract (locate, write, enumerate,
/// delete). Turning a decoded ``StagedCapture`` into library rows is the app's
/// job (it drives the Rust core); the extension has no database access.
struct StagingInbox {
    /// The `.../staging/inbox` directory this instance manages.
    let directory: URL

    private let fileManager: FileManager

    /// Errors raised while staging a capture from the extension.
    enum StagingError: Error {
        /// The shared App Group container is not available (unprovisioned
        /// entitlement), so there is nowhere shared to write.
        case appGroupUnavailable
    }

    init(directory: URL, fileManager: FileManager = .default) {
        self.directory = directory
        self.fileManager = fileManager
    }

    /// Resolves the shared drop folder in the App Group container, or `nil` when
    /// the App Group is not provisioned.
    static func shared(fileManager: FileManager = .default) -> StagingInbox? {
        guard let dir = AppGroup.stagingInboxURL(fileManager: fileManager) else { return nil }
        return StagingInbox(directory: dir, fileManager: fileManager)
    }

    // MARK: - Writer (share extension)

    /// Atomically publishes one capture as `<capture_id>.json`.
    ///
    /// Writes to a `.json.tmp` sibling first and then renames it into place, so
    /// the file only ever appears complete. Creates the drop folder on demand.
    func write(_ capture: StagedCapture) throws {
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(capture)

        let finalURL = directory.appendingPathComponent("\(capture.captureID).json")
        let tempURL = directory.appendingPathComponent("\(capture.captureID).json.tmp")

        // `.atomic` already writes-then-renames, but we stage through an explicit
        // `.tmp` so a crash leaves a name the reader skips (never a half `.json`).
        try data.write(to: tempURL, options: .atomic)
        if fileManager.fileExists(atPath: finalURL.path) {
            try fileManager.removeItem(at: finalURL)
        }
        try fileManager.moveItem(at: tempURL, to: finalURL)
    }

    // MARK: - Reader / drainer (main app)

    /// All pending, *readable* captures, sorted oldest-first by `capturedAt`.
    ///
    /// Only `.json` files are considered (`.tmp` leftovers are ignored).
    /// Malformed or future-versioned records are skipped and **left in place**
    /// (surfaced for diagnostics rather than dropped), per ADR-021.
    func pending() -> [PendingCapture] {
        guard
            let entries = try? fileManager.contentsOfDirectory(
                at: directory,
                includingPropertiesForKeys: nil,
                options: [.skipsHiddenFiles]
            )
        else {
            return []
        }

        let decoder = JSONDecoder()
        let records: [PendingCapture] = entries
            .filter { $0.pathExtension == "json" }
            .compactMap { url in
                guard
                    let data = try? Data(contentsOf: url),
                    let capture = try? decoder.decode(StagedCapture.self, from: data),
                    capture.isReadable
                else {
                    return nil
                }
                return PendingCapture(url: url, capture: capture)
            }

        return records.sorted { $0.capture.capturedAt < $1.capture.capturedAt }
    }

    /// Deletes a drained record. Called only after its database write commits,
    /// so a crash before this leaves the file to be reprocessed idempotently.
    func remove(at url: URL) {
        try? fileManager.removeItem(at: url)
    }
}

/// A staged record paired with the file it came from, so the drainer can delete
/// exactly that file after committing it.
struct PendingCapture {
    let url: URL
    let capture: StagedCapture
}
