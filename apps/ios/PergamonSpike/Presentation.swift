import SwiftUI

// `ContentItem` already exposes a `String` id from Rust, so Identifiable
// conformance is free and lets us drive SwiftUI `List`/`ForEach` directly.
extension ContentItem: Identifiable {}

extension Status {
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

    var systemImage: String {
        switch self {
        case .inbox: return "tray"
        case .later: return "clock"
        case .reference: return "bookmark"
        case .reading: return "book"
        case .archived: return "archivebox"
        case .discarded: return "trash"
        }
    }

    var tint: Color {
        switch self {
        case .inbox: return .blue
        case .later: return .orange
        case .reference: return .purple
        case .reading: return .green
        case .archived: return .gray
        case .discarded: return .red
        }
    }
}

extension ContentType {
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

    var systemImage: String {
        switch self {
        case .feedItem: return "dot.radiowaves.up.forward"
        case .article: return "doc.richtext"
        case .bookmark: return "bookmark.fill"
        case .highlight: return "highlighter"
        case .pdf: return "doc.fill"
        case .podcastEpisode: return "waveform"
        }
    }
}

extension ContentItem {
    /// Publication date derived from the Rust-provided epoch milliseconds.
    var publishedDate: Date? {
        publishedAtMillis.map { Date(timeIntervalSince1970: Double($0) / 1000.0) }
    }
}
