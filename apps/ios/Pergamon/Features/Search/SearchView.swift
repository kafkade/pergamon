import SwiftUI
import PergamonKit

/// The Search tab: full-text-ish lookup over the library via
/// `library.search(query:)`. An empty query shows a prompt; a non-matching
/// query shows an empty state. Tapping a result opens it in `DetailView`.
struct SearchView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @State private var query: String = ""

    private var trimmed: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var results: [ContentItem] {
        trimmed.isEmpty ? [] : environment.library.search(query: query)
    }

    var body: some View {
        NavigationStack {
            Group {
                if trimmed.isEmpty {
                    ContentUnavailableView(
                        "Search your library",
                        systemImage: "magnifyingglass",
                        description: Text("Find items by title, author, excerpt, or URL.")
                    )
                } else if results.isEmpty {
                    ContentUnavailableView.search(text: trimmed)
                } else {
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
            }
            .navigationTitle("Search")
            .navigationDestination(for: ContentItem.self) { item in
                DetailView(library: environment.library, itemID: item.id)
            }
            .searchable(text: $query, prompt: "Search pergamon")
        }
    }
}

#Preview {
    SearchView()
        .environmentObject(AppEnvironment())
}
