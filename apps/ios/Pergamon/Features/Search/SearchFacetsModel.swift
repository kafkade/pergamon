import Foundation
import PergamonKit

/// UI-side selection of the search facets, mirroring the CLI/web facet set
/// (`type / tag / status / source / since`). Lowered to a `PergamonKit`
/// `SearchFacets` value that drives `Library.searchFiltered`.
struct SearchFacetSelection: Equatable {
    var contentType: ContentType?
    var status: Status?
    var tag: String?
    var source: String?
    var publishedWithin: DatePreset = .anyTime

    /// Whether any facet is currently narrowing results.
    var isActive: Bool {
        contentType != nil
            || status != nil
            || tag != nil
            || source != nil
            || publishedWithin != .anyTime
    }

    /// The number of active facets, for a badge on the filter control.
    var activeCount: Int {
        var count = 0
        if contentType != nil { count += 1 }
        if status != nil { count += 1 }
        if tag != nil { count += 1 }
        if source != nil { count += 1 }
        if publishedWithin != .anyTime { count += 1 }
        return count
    }

    /// Lowers the selection to the FFI `SearchFacets` value.
    func facets(now: Date = Date()) -> SearchFacets {
        SearchFacets(
            contentType: contentType,
            status: status,
            tag: tag,
            source: source,
            sinceMillis: publishedWithin.sinceMillis(now: now),
            beforeMillis: nil
        )
    }

    /// Clears every facet back to its unconstrained default.
    mutating func clear() {
        self = SearchFacetSelection()
    }
}

/// A coarse "published within" preset for the date facet. Relative to now, so
/// it maps to a `sinceMillis` lower bound (inclusive).
enum DatePreset: String, CaseIterable, Identifiable {
    case anyTime = "Any time"
    case week = "Past week"
    case month = "Past month"
    case year = "Past year"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .anyTime: return "calendar"
        case .week: return "calendar.badge.clock"
        case .month: return "calendar.badge.clock"
        case .year: return "calendar.badge.clock"
        }
    }

    /// The inclusive lower bound as Unix epoch milliseconds, or `nil` for
    /// `.anyTime`.
    func sinceMillis(now: Date) -> Int64? {
        let days: Int
        switch self {
        case .anyTime: return nil
        case .week: days = 7
        case .month: days = 30
        case .year: days = 365
        }
        let since = now.addingTimeInterval(-Double(days) * 86_400)
        return Int64(since.timeIntervalSince1970 * 1000)
    }
}

/// The fixed catalogue of content types offered as a facet (UniFFI enums are not
/// `CaseIterable`, so we enumerate them explicitly).
enum ContentTypeCatalog {
    static let all: [ContentType] = [
        .article, .feedItem, .bookmark, .pdf, .highlight, .podcastEpisode,
    ]
}

/// The fixed catalogue of triage statuses offered as a facet.
enum StatusCatalog {
    static let all: [Status] = [
        .inbox, .later, .reference, .reading, .archived, .discarded,
    ]
}
