import Foundation

/// One staged share-sheet capture — the on-disk JSON contract from **ADR-021**.
///
/// The share extension serializes exactly one of these per capture to an atomic
/// drop file in the shared App Group container; the main app decodes it on next
/// launch/foreground and hands it to the Rust core for finalization. This type
/// is deliberately **independent of the UniFFI-generated `ShareCapture`**: it is
/// the wire/disk format (stable field names, `schema_version` for forward
/// compatibility), which the app maps onto the FFI record at ingestion time. The
/// extension never links the Rust core, so it can only speak this format.
struct StagedCapture: Codable, Equatable {
    /// The staging format version. Bump when the schema changes; readers skip
    /// records with a higher version rather than dropping data (ADR-021).
    static let currentSchemaVersion = 1

    /// Staging format version this record was written with.
    var schemaVersion: Int
    /// Stable UUID for this capture; also the drop-file name and the idempotency
    /// key for finalization.
    var captureID: String
    /// Capture time as Unix epoch milliseconds (ADR-019 time mapping). The app
    /// drains oldest-first by this value.
    var capturedAt: Int64
    /// The kind of capture — a finalization hint (see ``StagedContentKind``).
    var contentKind: StagedContentKind
    /// The raw shared URL (not yet canonicalized). Present for `.url` /
    /// `.urlWithSelection`.
    var url: String?
    /// The shared / selected text. Present for `.urlWithSelection` / `.text`.
    var selectedText: String?
    /// Title supplied by the share sheet (e.g. Safari's page title), stored
    /// without a fetch.
    var pageTitle: String?
    /// Best-effort originating bundle id, for provenance.
    var sourceApp: String?

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case captureID = "capture_id"
        case capturedAt = "captured_at"
        case contentKind = "content_kind"
        case url
        case selectedText = "selected_text"
        case pageTitle = "page_title"
        case sourceApp = "source_app"
    }

    /// Builds a capture stamped with the current schema version and time.
    init(
        captureID: String = UUID().uuidString,
        capturedAt: Int64 = Int64(Date().timeIntervalSince1970 * 1000),
        contentKind: StagedContentKind,
        url: String? = nil,
        selectedText: String? = nil,
        pageTitle: String? = nil,
        sourceApp: String? = nil
    ) {
        self.schemaVersion = Self.currentSchemaVersion
        self.captureID = captureID
        self.capturedAt = capturedAt
        self.contentKind = contentKind
        self.url = url
        self.selectedText = selectedText
        self.pageTitle = pageTitle
        self.sourceApp = sourceApp
    }

    /// Whether this record is from a schema version this build understands.
    /// Records from a *newer* extension are skipped (left in place) rather than
    /// ingested, per ADR-021's forward-compatibility rule.
    var isReadable: Bool { schemaVersion <= Self.currentSchemaVersion }
}

/// What a staged capture carries, mirroring ADR-021's `content_kind`
/// discriminator and the Rust-side `ShareContentKind`.
enum StagedContentKind: String, Codable, Equatable {
    /// A bare URL shared from Safari or another app.
    case url
    /// A URL shared together with a text selection from the page.
    case urlWithSelection = "url_with_selection"
    /// A standalone text selection with no source URL.
    case text
}
