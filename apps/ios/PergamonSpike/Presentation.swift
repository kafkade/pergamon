import SwiftUI
import PergamonKit

// App-layer, SwiftUI-specific styling for the model types. Pure model
// conveniences (`Identifiable`, `publishedDate`, `label`) live in PergamonKit;
// only presentation concerns that depend on SwiftUI (SF Symbols, `Color`) stay
// here.

extension Status {
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
