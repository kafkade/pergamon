import SwiftUI
import PergamonKit

/// Filter options for status browsing: "All" plus every triage status. Shared
/// with `InboxView`.
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

/// Top-level browse mode for the Saved tab.
enum SavedScope: String, CaseIterable, Identifiable {
    case all = "All"
    case status = "Status"
    case collections = "Collections"
    case tags = "Tags"

    var id: String { rawValue }
}

/// The Saved tab: the wider library across every triage state, plus organization
/// browsing. A scope picker switches between the flat list (`All`), status
/// filtering (`Status`), the nested collection tree (`Collections`), and the tag
/// index (`Tags`). Every path ultimately drills into `DetailView`.
struct SavedView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @State private var scope: SavedScope = .all
    @State private var statusFilter: StatusFilter = .status(.inbox)
    /// Bumped when the library is replaced (backup restore) to force the
    /// live-reading `content` to re-render from the core.
    @State private var reloadTick = 0

    private var library: Library { environment.library }

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Saved")
                .navigationDestination(for: ContentItem.self) { item in
                    DetailView(library: library, itemID: item.id)
                }
                .safeAreaInset(edge: .top) { scopePicker }
                .toolbar {
                    if scope == .status {
                        statusMenu
                    }
                }
                .id(reloadTick)
        }
        .onReceive(NotificationCenter.default.publisher(for: .pergamonLibraryDidChange)) { _ in
            reloadTick += 1
        }
    }

    @ViewBuilder
    private var content: some View {
        switch scope {
        case .all:
            itemList(library.items(), emptyLabel: "Nothing saved", emptyDetail: "Saved items appear here.")
        case .status:
            itemList(
                library.itemsWithStatus(status: statusStatus),
                emptyLabel: "Nothing here",
                emptyDetail: "No items are “\(statusFilter.label)”."
            )
        case .collections:
            collectionsList
        case .tags:
            tagsList
        }
    }

    /// The concrete status backing the current status filter (defaults to inbox
    /// when "All" is chosen, which the picker only offers under `.status`).
    private var statusStatus: Status {
        if case .status(let status) = statusFilter { return status }
        return .inbox
    }

    // MARK: - Item lists

    private func itemList(_ items: [ContentItem], emptyLabel: String, emptyDetail: String) -> some View {
        Group {
            if items.isEmpty {
                ContentUnavailableView(
                    emptyLabel,
                    systemImage: "square.stack.3d.up",
                    description: Text(emptyDetail)
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
    }

    // MARK: - Collections

    private var collectionsList: some View {
        let collections = library.collections()
        return Group {
            if collections.isEmpty {
                ContentUnavailableView(
                    "No collections",
                    systemImage: Collection.systemImage,
                    description: Text("Group items into collections from any item’s detail view.")
                )
            } else {
                List(collections) { collection in
                    NavigationLink {
                        CollectionItemsView(library: library, collection: collection)
                    } label: {
                        CollectionRowLabel(collection: collection)
                    }
                }
                .listStyle(.insetGrouped)
            }
        }
    }

    // MARK: - Tags

    private var tagsList: some View {
        let tags = library.tags()
        return Group {
            if tags.isEmpty {
                ContentUnavailableView(
                    "No tags",
                    systemImage: Tag.systemImage,
                    description: Text("Tag items from any item’s detail view.")
                )
            } else {
                List(tags) { tag in
                    NavigationLink {
                        TagItemsView(library: library, tagName: tag.name)
                    } label: {
                        Label {
                            HStack {
                                Text("#\(tag.name)")
                                Spacer()
                                Text("\(tag.itemCount)")
                                    .foregroundStyle(.secondary)
                                    .monospacedDigit()
                            }
                        } icon: {
                            Image(systemName: Tag.systemImage)
                        }
                    }
                }
                .listStyle(.insetGrouped)
            }
        }
    }

    // MARK: - Toolbars

    private var scopePicker: some View {
        Picker("Scope", selection: $scope) {
            ForEach(SavedScope.allCases) { option in
                Text(option.rawValue).tag(option)
            }
        }
        .pickerStyle(.segmented)
        .padding(.horizontal)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private var statusMenu: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Picker("Status", selection: $statusFilter) {
                    // Only the concrete statuses; the flat list lives under "All".
                    ForEach(StatusFilter.allCases.filter { $0 != .all }) { option in
                        Label(option.label, systemImage: option.systemImage).tag(option)
                    }
                }
            } label: {
                Label("Status", systemImage: "line.3.horizontal.decrease.circle")
            }
        }
    }
}

/// A row in the collection tree: indented by nesting depth, with a direct item
/// count.
struct CollectionRowLabel: View {
    let collection: Collection

    var body: some View {
        Label {
            HStack {
                Text(collection.name)
                Spacer()
                Text("\(collection.itemCount)")
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
        } icon: {
            Image(systemName: collection.isNested ? "folder" : "folder.fill")
        }
        .padding(.leading, CGFloat(collection.depth) * 16)
    }
}

/// The items directly in a collection, drilled into from the collection tree.
struct CollectionItemsView: View {
    let library: Library
    let collection: Collection

    @State private var items: [ContentItem] = []
    @State private var loadError: String?

    var body: some View {
        Group {
            if let loadError {
                ContentUnavailableView(
                    "Couldn’t load",
                    systemImage: "exclamationmark.triangle",
                    description: Text(loadError)
                )
            } else if items.isEmpty {
                ContentUnavailableView(
                    "Empty collection",
                    systemImage: Collection.systemImage,
                    description: Text("No items in “\(collection.name)” yet.")
                )
            } else {
                List(items) { item in
                    NavigationLink(value: item) {
                        ItemRow(item: item)
                    }
                }
                .listStyle(.plain)
            }
        }
        .navigationTitle(collection.name)
        .navigationBarTitleDisplayMode(.inline)
        .onAppear(perform: load)
    }

    private func load() {
        do {
            items = try library.itemsInCollection(collectionId: collection.id)
            loadError = nil
        } catch {
            loadError = "\(error)"
        }
    }
}

/// The items carrying a tag, drilled into from the tag index.
struct TagItemsView: View {
    let library: Library
    let tagName: String

    private var items: [ContentItem] {
        library.itemsWithTag(tag: tagName)
    }

    var body: some View {
        Group {
            if items.isEmpty {
                ContentUnavailableView(
                    "No items",
                    systemImage: Tag.systemImage,
                    description: Text("Nothing is tagged #\(tagName).")
                )
            } else {
                List(items) { item in
                    NavigationLink(value: item) {
                        ItemRow(item: item)
                    }
                }
                .listStyle(.plain)
            }
        }
        .navigationTitle("#\(tagName)")
        .navigationBarTitleDisplayMode(.inline)
    }
}

#Preview {
    SavedView()
        .environmentObject(AppEnvironment())
}
