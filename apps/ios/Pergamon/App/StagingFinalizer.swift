import Foundation
import PergamonKit

/// Drains the share-extension staging drop folder into the library, per
/// **ADR-021**'s finalization flow.
///
/// The share extension only *stages* captures (atomic JSON files); this is the
/// app-side finalizer that turns each one into real library rows. It processes
/// records **oldest-first**, maps each decoded ``StagedCapture`` onto the Rust
/// core's `ShareCapture` FFI record, runs the shared ingestion pipeline
/// (`Library.ingestShareCapture` — canonicalize → dedupe → create/enrich →
/// attach highlight), and deletes the file **only after** the write commits. A
/// crash between the commit and the delete simply reprocesses the survivor; the
/// core's dedupe (canonical URL, or `capture_id` for text) makes that converge
/// instead of duplicate.
struct StagingFinalizer {
    private let library: Library
    private let inbox: StagingInbox

    init(library: Library, inbox: StagingInbox) {
        self.library = library
        self.inbox = inbox
    }

    /// The outcome of one drain pass.
    struct Result {
        /// Number of records that committed to the library (new or merged).
        var ingested = 0
        /// Number of records left in place after a failure, for a later retry.
        var failed = 0

        /// Whether the library changed, so surfaces should reload.
        var didChange: Bool { ingested > 0 }
    }

    /// Ingests every pending capture and returns what happened. Safe to call
    /// repeatedly (on launch and on every foreground); an empty folder is a
    /// no-op.
    @discardableResult
    func drain() -> Result {
        var result = Result()

        for pending in inbox.pending() {
            do {
                _ = try library.ingestShareCapture(capture: Self.ffi(pending.capture))
                // Commit succeeded: only now is it safe to drop the file.
                inbox.remove(at: pending.url)
                result.ingested += 1
            } catch {
                // Leave the file in place so the next drain retries it. A
                // genuinely poisoned record keeps failing but never corrupts the
                // library or blocks the others.
                print("[staging] failed to finalize \(pending.url.lastPathComponent): \(error)")
                result.failed += 1
            }
        }

        return result
    }

    /// Maps the on-disk staging record onto the UniFFI `ShareCapture` the core
    /// ingests. The two types are intentionally separate (disk contract vs. FFI
    /// contract); this is the single crossing point.
    private static func ffi(_ staged: StagedCapture) -> ShareCapture {
        ShareCapture(
            captureId: staged.captureID,
            capturedAtMillis: staged.capturedAt,
            contentKind: ffiKind(staged.contentKind),
            url: staged.url,
            selectedText: staged.selectedText,
            pageTitle: staged.pageTitle,
            sourceApp: staged.sourceApp
        )
    }

    private static func ffiKind(_ kind: StagedContentKind) -> ShareContentKind {
        switch kind {
        case .url: .url
        case .urlWithSelection: .urlWithSelection
        case .text: .text
        }
    }
}
