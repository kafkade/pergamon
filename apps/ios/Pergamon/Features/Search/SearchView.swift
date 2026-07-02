import SwiftUI
import PergamonKit

/// The Search tab: full-text search over the library via
/// `library.searchFiltered(query:facets:)`, with faceted filters (content type,
/// status, tag, source, and a published-within date preset) that mirror the
/// CLI/web facet set so results stay consistent across clients on the same
/// library.
///
/// An empty query with no active facets shows a prompt; an empty query with
/// active facets still filters. Tapping a result opens it in `DetailView`.
struct SearchView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @State private var query: String = ""
    @State private var facets = SearchFacetSelection()

    private var library: Library { environment.library }

    private var trimmed: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var isEmptySearch: Bool {
        trimmed.isEmpty && !facets.isActive
    }

    private var results: [ContentItem] {
        isEmptySearch ? [] : library.searchFiltered(query: query, facets: facets.facets())
    }

    var body: some View {
        NavigationStack {
            Group {
                if isEmptySearch {
                    ContentUnavailableView(
                        "Search your library",
                        systemImage: "magnifyingglass",
                        description: Text("Find items by title, author, excerpt, URL, or content — then narrow with filters.")
                    )
                } else if results.isEmpty {
                    ContentUnavailableView(
                        "No results",
                        systemImage: "magnifyingglass",
                        description: Text(emptyResultsDescription)
                    )
                } else {
                    resultsList
                }
            }
            .safeAreaInset(edge: .top) {
                if facets.isActive {
                    activeFacetsBar
                }
            }
            .navigationTitle("Search")
            .navigationDestination(for: ContentItem.self) { item in
                DetailView(library: library, itemID: item.id)
            }
            .searchable(text: $query, prompt: "Search pergamon")
            .toolbar { filterMenu }
        }
    }

    private var resultsList: some View {
        List {
            Section {
                ForEach(results) { item in
                    NavigationLink(value: item) {
                        ItemRow(item: item)
                    }
                }
            } footer: {
                Text("\(results.count) result(s) · pergamon-core \(environment.coreVersion) via UniFFI")
                    .font(.footnote)
            }
        }
        .listStyle(.plain)
    }

    // MARK: - Facets

    private var filterMenu: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Picker("Type", selection: $facets.contentType) {
                    Label("Any Type", systemImage: "square.grid.2x2").tag(ContentType?.none)
                    ForEach(ContentTypeCatalog.all, id: \.self) { type in
                        Label(type.label, systemImage: type.systemImage)
                            .tag(ContentType?.some(type))
                    }
                }

                Picker("Status", selection: $facets.status) {
                    Label("Any Status", systemImage: "square.stack.3d.up").tag(Status?.none)
                    ForEach(StatusCatalog.all, id: \.self) { status in
                        Label(status.label, systemImage: status.systemImage)
                            .tag(Status?.some(status))
                    }
                }

                let tags = library.tags()
                if !tags.isEmpty {
                    Picker("Tag", selection: $facets.tag) {
                        Label("Any Tag", systemImage: Tag.systemImage).tag(String?.none)
                        ForEach(tags) { tag in
                            Text("#\(tag.name)").tag(String?.some(tag.name))
                        }
                    }
                }

                let sources = library.sources()
                if !sources.isEmpty {
                    Picker("Source", selection: $facets.source) {
                        Label("Any Source", systemImage: "dot.radiowaves.up.forward")
                            .tag(String?.none)
                        ForEach(sources, id: \.self) { source in
                            Text(source).tag(String?.some(source))
                        }
                    }
                }

                Picker("Published", selection: $facets.publishedWithin) {
                    ForEach(DatePreset.allCases) { preset in
                        Label(preset.rawValue, systemImage: preset.systemImage).tag(preset)
                    }
                }

                if facets.isActive {
                    Divider()
                    Button(role: .destructive) {
                        facets.clear()
                    } label: {
                        Label("Clear Filters", systemImage: "xmark.circle")
                    }
                }
            } label: {
                Label(
                    "Filter",
                    systemImage: facets.isActive
                        ? "line.3.horizontal.decrease.circle.fill"
                        : "line.3.horizontal.decrease.circle"
                )
            }
        }
    }

    /// A horizontally scrolling row of chips summarizing the active facets, each
    /// removable by tapping its ✕.
    private var activeFacetsBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                if let type = facets.contentType {
                    facetChip(type.label, systemImage: type.systemImage) { facets.contentType = nil }
                }
                if let status = facets.status {
                    facetChip(status.label, systemImage: status.systemImage) { facets.status = nil }
                }
                if let tag = facets.tag {
                    facetChip("#\(tag)", systemImage: Tag.systemImage) { facets.tag = nil }
                }
                if let source = facets.source {
                    facetChip(source, systemImage: "dot.radiowaves.up.forward") { facets.source = nil }
                }
                if facets.publishedWithin != .anyTime {
                    facetChip(facets.publishedWithin.rawValue, systemImage: "calendar") {
                        facets.publishedWithin = .anyTime
                    }
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 8)
        }
        .background(.bar)
    }

    private func facetChip(
        _ text: String,
        systemImage: String,
        onRemove: @escaping () -> Void
    ) -> some View {
        HStack(spacing: 4) {
            Image(systemName: systemImage)
            Text(text)
            Button(action: onRemove) {
                Image(systemName: "xmark.circle.fill")
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(text) filter")
        }
        .font(.caption)
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Color.accentColor.opacity(0.12), in: Capsule())
        .foregroundStyle(Color.accentColor)
    }

    private var emptyResultsDescription: String {
        if trimmed.isEmpty {
            return "No items match the active filters."
        }
        return facets.isActive
            ? "No items match “\(trimmed)” with the active filters."
            : "No items match “\(trimmed)”."
    }
}

#Preview {
    SearchView()
        .environmentObject(AppEnvironment())
}
