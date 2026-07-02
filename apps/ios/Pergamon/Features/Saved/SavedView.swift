import SwiftUI
import PergamonKit

/// Filter options for the Saved list: "All" plus every triage status.
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

/// The Saved tab: the wider library across every triage state, filterable by
/// status. Tapping a row opens it in `DetailView`.
struct SavedView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @State private var filter: StatusFilter = .all

    private var items: [ContentItem] {
        switch filter {
        case .all:
            return environment.library.items()
        case .status(let status):
            return environment.library.itemsWithStatus(status: status)
        }
    }

    var body: some View {
        NavigationStack {
            Group {
                if items.isEmpty {
                    ContentUnavailableView(
                        "Nothing saved",
                        systemImage: "square.stack.3d.up",
                        description: Text("No items match “\(filter.label)”.")
                    )
                } else {
                    List {
                        Section {
                            ForEach(items) { item in
                                NavigationLink(value: item) {
                                    ItemRow(item: item)
                                }
                            }
                        } footer: {
                            Text("\(items.count) item(s) · pergamon-core \(environment.coreVersion) via UniFFI")
                                .font(.footnote)
                        }
                    }
                    .listStyle(.plain)
                }
            }
            .navigationTitle("Saved")
            .navigationDestination(for: ContentItem.self) { item in
                DetailView(library: environment.library, itemID: item.id)
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

#Preview {
    SavedView()
        .environmentObject(AppEnvironment())
}
