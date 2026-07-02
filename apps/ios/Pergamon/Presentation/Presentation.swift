import SwiftUI
import PergamonKit

// App-layer, SwiftUI-specific styling for the core model types. Pure model
// conveniences (`Identifiable`, `publishedDate`, `label`) live in PergamonKit;
// only presentation concerns that depend on SwiftUI (SF Symbols, `Color`) stay
// here.

extension Status {
    /// SF Symbol representing this triage status.
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

    /// Accent color used for this status across rows and badges.
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
    /// SF Symbol representing this content type.
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

extension Tag {
    /// SF Symbol used to represent a tag across the app.
    static var systemImage: String { "tag" }
}

extension Collection {
    /// SF Symbol used to represent a collection across the app.
    static var systemImage: String { "folder" }
}

extension ReviewGrade {
    /// Accent color used for this grade's button in the review queue. Runs
    /// red → orange → blue → green as recall improves, matching the FSRS
    /// Again/Hard/Good/Easy scale.
    var tint: Color {
        switch self {
        case .again: return .red
        case .hard: return .orange
        case .good: return .blue
        case .easy: return .green
        }
    }

    /// SF Symbol representing this grade.
    var systemImage: String {
        switch self {
        case .again: return "arrow.counterclockwise"
        case .hard: return "tortoise"
        case .good: return "checkmark"
        case .easy: return "hare"
        }
    }
}

extension ReviewState {
    /// Accent color used for the card's state badge.
    var tint: Color {
        switch self {
        case .new: return .blue
        case .learning: return .orange
        case .review: return .green
        case .relearning: return .red
        }
    }
}
