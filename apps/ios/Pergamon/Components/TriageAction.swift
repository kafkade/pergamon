import SwiftUI
import PergamonKit

/// A triage action the user can take on a content item, shared by the inbox
/// swipe actions and the reader toolbar so both surfaces stay in lock-step.
///
/// Each case knows its label, SF Symbol, tint, and how to apply itself to the
/// Rust `Library`. The library mutates the local in-memory corpus synchronously
/// (ADR-019) and returns the updated item; persistence across launches lands
/// with the SQLite store (#118).
enum TriageAction: Identifiable, CaseIterable {
    case markRead
    case markUnread
    case saveForLater
    case archive

    var id: String { label }

    var label: String {
        switch self {
        case .markRead: return "Mark Read"
        case .markUnread: return "Mark Unread"
        case .saveForLater: return "Save for Later"
        case .archive: return "Archive"
        }
    }

    var systemImage: String {
        switch self {
        case .markRead: return "checkmark.circle"
        case .markUnread: return "circle"
        case .saveForLater: return "clock"
        case .archive: return "archivebox"
        }
    }

    var tint: Color {
        switch self {
        case .markRead: return .blue
        case .markUnread: return .gray
        case .saveForLater: return .orange
        case .archive: return .green
        }
    }

    /// The read/unread toggle appropriate for `item`'s current state.
    static func readToggle(for item: ContentItem) -> TriageAction {
        item.isRead ? .markUnread : .markRead
    }

    /// Applies the action to the item with `id`, returning the updated item.
    ///
    /// Errors are surfaced to the caller so the UI can decide how to react; the
    /// ids handled here come from live items, so failures are not expected in
    /// normal use.
    @discardableResult
    func apply(to id: String, using library: Library) throws -> ContentItem {
        switch self {
        case .markRead: return try library.markRead(id: id)
        case .markUnread: return try library.markUnread(id: id)
        case .saveForLater: return try library.saveForLater(id: id)
        case .archive: return try library.archive(id: id)
        }
    }
}
