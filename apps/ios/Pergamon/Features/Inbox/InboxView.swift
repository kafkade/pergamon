import SwiftUI
import PergamonKit

/// Read/unread filter for the inbox list.
enum ReadFilter: String, CaseIterable, Identifiable {
    case all = "All"
    case unread = "Unread"
    case read = "Read"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .all: return "circle.lefthalf.filled"
        case .unread: return "circle.fill"
        case .read: return "checkmark.circle"
        }
    }

    func matches(_ item: ContentItem) -> Bool {
        switch self {
        case .all: return true
        case .unread: return !item.isRead
        case .read: return item.isRead
        }
    }
}

/// The inbox tab: newly-captured items awaiting triage, the app's primary
/// landing screen. Supports status, feed/source, and read/unread filtering, and
/// per-row swipe actions (mark read/unread, save for later, archive) that mutate
/// the Rust core and refresh in place. Tapping a row opens it in the reader.
struct InboxView: View {
    @EnvironmentObject private var environment: AppEnvironment

    @State private var items: [ContentItem] = []
    @State private var statusFilter: StatusFilter = .status(.inbox)
    @State private var sourceFilter: String?
    @State private var readFilter: ReadFilter = .all

    private var library: Library { environment.library }

    var body: some View {
        NavigationStack {
            Group {
                if items.isEmpty {
                    ContentUnavailableView(
                        "Inbox zero",
                        systemImage: "tray",
                        description: Text(emptyDescription)
                    )
                } else {
                    list
                }
            }
            .navigationTitle("Inbox")
            .navigationDestination(for: ContentItem.self) { item in
                DetailView(library: library, itemID: item.id)
            }
            .toolbar { filterMenu }
        }
        .onAppear(perform: reload)
        .onChange(of: statusFilter) { reload() }
        .onChange(of: sourceFilter) { reload() }
        .onChange(of: readFilter) { reload() }
    }

    private var list: some View {
        List {
            Section {
                ForEach(items) { item in
                    NavigationLink(value: item) {
                        ItemRow(item: item)
                    }
                    .swipeActions(edge: .leading, allowsFullSwipe: true) {
                        let toggle = TriageAction.readToggle(for: item)
                        Button {
                            perform(toggle, on: item)
                        } label: {
                            Label(toggle.label, systemImage: toggle.systemImage)
                        }
                        .tint(toggle.tint)
                    }
                    .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                        Button {
                            perform(.archive, on: item)
                        } label: {
                            Label(TriageAction.archive.label, systemImage: TriageAction.archive.systemImage)
                        }
                        .tint(TriageAction.archive.tint)

                        Button {
                            perform(.saveForLater, on: item)
                        } label: {
                            Label(TriageAction.saveForLater.label, systemImage: TriageAction.saveForLater.systemImage)
                        }
                        .tint(TriageAction.saveForLater.tint)
                    }
                }
            } footer: {
                Text("\(items.count) item(s) · pergamon-core \(environment.coreVersion) via UniFFI")
                    .font(.footnote)
            }
        }
        .listStyle(.plain)
    }

    private var filterMenu: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Picker("Status", selection: $statusFilter) {
                    ForEach(StatusFilter.allCases) { option in
                        Label(option.label, systemImage: option.systemImage).tag(option)
                    }
                }

                Picker("Read", selection: $readFilter) {
                    ForEach(ReadFilter.allCases) { option in
                        Label(option.rawValue, systemImage: option.systemImage).tag(option)
                    }
                }

                let sources = library.sources()
                if !sources.isEmpty {
                    Picker("Feed", selection: $sourceFilter) {
                        Label("All Feeds", systemImage: "dot.radiowaves.up.forward")
                            .tag(String?.none)
                        ForEach(sources, id: \.self) { source in
                            Text(source).tag(String?.some(source))
                        }
                    }
                }
            } label: {
                Label(
                    "Filter",
                    systemImage: filtersActive
                        ? "line.3.horizontal.decrease.circle.fill"
                        : "line.3.horizontal.decrease.circle"
                )
            }
        }
    }

    private var filtersActive: Bool {
        statusFilter != .status(.inbox) || sourceFilter != nil || readFilter != .all
    }

    private var emptyDescription: String {
        filtersActive
            ? "No items match the current filters."
            : "Nothing to triage. Saved items land here first."
    }

    /// Reloads the visible list from the core, applying the status filter in
    /// Rust and the feed/read filters in Swift.
    private func reload() {
        let base: [ContentItem]
        switch statusFilter {
        case .all:
            base = library.items()
        case .status(let status):
            base = library.itemsWithStatus(status: status)
        }
        items = base.filter { item in
            (sourceFilter == nil || item.sourceName == sourceFilter)
                && readFilter.matches(item)
        }
    }

    /// Applies a triage action to `item` then refreshes the list.
    private func perform(_ action: TriageAction, on item: ContentItem) {
        do {
            try action.apply(to: item.id, using: library)
        } catch {
            print("[inbox] \(action.label) failed for \(item.id): \(error)")
        }
        reload()
    }
}

#Preview {
    InboxView()
        .environmentObject(AppEnvironment())
}
