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
