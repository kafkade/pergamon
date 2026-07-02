import SwiftUI
import PergamonKit

/// The inbox: lists items returned by the Rust core and lets you filter by
/// triage status. Tapping a row opens it (a fresh `library.item(id:)` round-trip
/// into Rust) in `DetailView`.
struct InboxView: View {
    @State private var filter: StatusFilter = .all

    /// The stateful entry point into the Rust core. Backed by an in-memory
    /// seeded corpus today; the on-device SQLite store lands with #118.
    private let library = Library()

    private var items: [ContentItem] {
        switch filter {
        case .all:
            return library.items()
        case .status(let status):
            return library.itemsWithStatus(status: status)
        }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(items) { item in
                        NavigationLink(value: item) {
                            ItemRow(item: item)
                        }
                    }
                } footer: {
                    Text("\(items.count) item(s) • served by pergamon-core \(libraryVersion()) via UniFFI")
                        .font(.footnote)
                }
            }
            .listStyle(.plain)
            .navigationTitle("pergamon")
            .navigationDestination(for: ContentItem.self) { item in
                DetailView(library: library, itemID: item.id)
            }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Picker("Filter", selection: $filter) {
                            ForEach(StatusFilter.allCases) { option in
                                Label(option.label, systemImage: option.systemImage)
                                    .tag(option)
                            }
                        }
                    } label: {
                        Label("Filter", systemImage: "line.3.horizontal.decrease.circle")
                    }
                }
            }
        }
    }
}

/// One row in the inbox list.
private struct ItemRow: View {
    let item: ContentItem

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: item.contentType.systemImage)
                .font(.title3)
                .foregroundStyle(item.status.tint)
                .frame(width: 28)

            VStack(alignment: .leading, spacing: 4) {
                Text(item.title)
                    .font(.headline)
                    .lineLimit(2)

                if let author = item.author {
                    Text(author)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 6) {
                    Label(item.status.label, systemImage: item.status.systemImage)
                        .labelStyle(.titleAndIcon)
                        .foregroundStyle(item.status.tint)
                    Text("·")
                    Text(item.contentType.label)
                    Text("·")
                    Text("\(item.readingMinutes) min read")
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 4)
    }
}

/// Filter options for the inbox, including "All" plus every triage status.
enum StatusFilter: Hashable, Identifiable, CaseIterable {
    case all
    case status(Status)

    static var allCases: [StatusFilter] {
        [.all] + [Status.inbox, .later, .reference, .reading, .archived, .discarded]
            .map(StatusFilter.status)
    }

    var id: String { label }

    var label: String {
        switch self {
        case .all: return "All"
        case .status(let status): return status.label
        }
    }

    var systemImage: String {
        switch self {
        case .all: return "square.stack.3d.up"
        case .status(let status): return status.systemImage
        }
    }
}

#Preview {
    InboxView()
}
