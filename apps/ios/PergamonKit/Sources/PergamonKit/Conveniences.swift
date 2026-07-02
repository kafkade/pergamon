import Foundation

// Model-level conveniences over the generated value types. These are pure,
// reusable, and UI-framework-agnostic (no SwiftUI). App-specific styling — SF
// Symbol names, tint colors — stays in the app layer.

extension ContentItem: Identifiable {
    // `ContentItem` already carries a stable `String` id from Rust, so
    // Identifiable conformance is free and drives SwiftUI `List` / `ForEach`
    // directly.
}

public extension ContentItem {
    /// Publication date derived from the Rust-provided epoch milliseconds, or
    /// `nil` when the item has no known publication time.
    var publishedDate: Date? {
        publishedAtMillis.map { Date(timeIntervalSince1970: Double($0) / 1000.0) }
    }

    /// When the item was marked read, derived from the Rust-provided epoch
    /// milliseconds, or `nil` when the item is unread.
    var readDate: Date? {
        readAtMillis.map { Date(timeIntervalSince1970: Double($0) / 1000.0) }
    }

    /// Whether the item has been read. Read state is tracked independently of
    /// triage `status` (archiving also marks an item read).
    var isRead: Bool {
        readAtMillis != nil
    }
}

public extension Status {
    /// Human-readable label for the triage status.
    var label: String {
        switch self {
        case .inbox: return "Inbox"
        case .later: return "Later"
        case .reference: return "Reference"
        case .reading: return "Reading"
        case .archived: return "Archived"
        case .discarded: return "Discarded"
        }
    }
}

public extension ContentType {
    /// Human-readable label for the content type.
    var label: String {
        switch self {
        case .feedItem: return "Feed"
        case .article: return "Article"
        case .bookmark: return "Bookmark"
        case .highlight: return "Highlight"
        case .pdf: return "PDF"
        case .podcastEpisode: return "Podcast"
        }
    }
}

extension Tag: Identifiable {
    // `Tag` already carries a stable `String` id from Rust, so Identifiable
    // conformance is free and drives SwiftUI `List` / `ForEach` directly.
}

extension Collection: Identifiable {
    // Same as `Tag`: the Rust-provided `id` backs Identifiable directly.
}

public extension Collection {
    /// Whether this collection is nested under a parent (`depth > 0`).
    var isNested: Bool { depth > 0 }
}

extension Highlight: Identifiable {
    // `Highlight` carries a stable `String` id from Rust; Identifiable is free.
}

public extension Highlight {
    /// When the highlight was captured, derived from the Rust-provided epoch
    /// milliseconds.
    var createdDate: Date {
        Date(timeIntervalSince1970: Double(createdAtMillis) / 1000.0)
    }
}

extension ReviewCardView: Identifiable {
    /// The card's own id drives Identifiable so SwiftUI can diff the review
    /// queue as cards are graded and rescheduled.
    public var id: String { cardId }
}

public extension ReviewCardView {
    /// When the card is next due, derived from the Rust-provided epoch millis.
    var dueDate: Date {
        Date(timeIntervalSince1970: Double(dueAtMillis) / 1000.0)
    }

    /// When the card was last reviewed, or `nil` if it never has been.
    var lastReviewedDate: Date? {
        lastReviewedAtMillis.map { Date(timeIntervalSince1970: Double($0) / 1000.0) }
    }

    /// Whether this card has never been reviewed.
    var isNew: Bool { reviewCount == 0 }
}

public extension ReviewGrade {
    /// Human-readable label for the grade button.
    var label: String {
        switch self {
        case .again: return "Again"
        case .hard: return "Hard"
        case .good: return "Good"
        case .easy: return "Easy"
        }
    }
}

public extension ReviewState {
    /// Human-readable label for the card's lifecycle state.
    var label: String {
        switch self {
        case .new: return "New"
        case .learning: return "Learning"
        case .review: return "Review"
        case .relearning: return "Relearning"
        }
    }
}

