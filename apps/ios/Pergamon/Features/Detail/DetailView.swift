import SwiftUI
import PergamonKit

/// Detail / reader view, shared by every list in the app.
///
/// It loads the item from Rust by id via `library.item(id:)` — a round-trip
/// lookup across the FFI boundary that throws `PergamonError` — then renders the
/// normalized extracted content (`contentText`) as a readable article. Because
/// the content is served entirely from the local core, the reader works offline;
/// no network path is involved. Triage actions (mark read/unread, save for
/// later, archive) mutate the core and refresh the view in place.
struct DetailView: View {
    let library: Library
    let itemID: String

    @State private var item: ContentItem?
    @State private var loadFailed = false

    var body: some View {
        Group {
            if let item {
                content(for: item)
            } else {
                ContentUnavailableView(
                    "Not found",
                    systemImage: "questionmark.folder",
                    description: Text("No item with id \(itemID)")
                )
            }
        }
        .navigationTitle("Reader")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let item {
                ToolbarItem(placement: .topBarTrailing) {
                    actionsMenu(for: item)
                }
            }
        }
        .onAppear(perform: load)
    }

    @ViewBuilder
    private func content(for item: ContentItem) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text(item.title)
                    .font(.title2.bold())

                HStack(spacing: 8) {
                    Label(item.status.label, systemImage: item.status.systemImage)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(item.status.tint.opacity(0.15), in: Capsule())
                        .foregroundStyle(item.status.tint)
                    Label(item.contentType.label, systemImage: item.contentType.systemImage)
                        .foregroundStyle(.secondary)
                    if item.isRead {
                        Label("Read", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.secondary)
                    }
                }
                .font(.caption)

                if let source = item.sourceName {
                    metadata(icon: "dot.radiowaves.up.forward", text: source)
                }
                if let author = item.author {
                    metadata(icon: "person", text: author)
                }
                if let date = item.publishedDate {
                    metadata(icon: "calendar", text: date.formatted(date: .abbreviated, time: .omitted))
                }
                metadata(icon: "clock", text: "\(item.readingMinutes) min read")
                if let url = item.url {
                    metadata(icon: "link", text: url)
                }

                Divider()

                articleBody(for: item)

                Divider()
                Label("Available offline · served from the local core", systemImage: "wifi.slash")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                Text("id \(item.id)")
                    .font(.caption2.monospaced())
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()
        }
    }

    /// The reader body: the normalized extracted text when present, falling back
    /// to the excerpt, then a placeholder. Tuned for comfortable reading.
    @ViewBuilder
    private func articleBody(for item: ContentItem) -> some View {
        if let text = item.contentText, !text.isEmpty {
            Text(text)
                .font(.body)
                .lineSpacing(6)
                .textSelection(.enabled)
        } else if let excerpt = item.excerpt {
            Text(excerpt)
                .font(.body)
                .italic()
                .foregroundStyle(.secondary)
        } else {
            Text("No extracted content for this item yet.")
                .font(.body)
                .foregroundStyle(.tertiary)
        }
    }

    private func actionsMenu(for item: ContentItem) -> some View {
        Menu {
            let toggle = TriageAction.readToggle(for: item)
            Button {
                perform(toggle)
            } label: {
                Label(toggle.label, systemImage: toggle.systemImage)
            }
            Button {
                perform(.saveForLater)
            } label: {
                Label(TriageAction.saveForLater.label, systemImage: TriageAction.saveForLater.systemImage)
            }
            Button {
                perform(.archive)
            } label: {
                Label(TriageAction.archive.label, systemImage: TriageAction.archive.systemImage)
            }
        } label: {
            Label("Actions", systemImage: "ellipsis.circle")
        }
    }

    private func metadata(icon: String, text: String) -> some View {
        Label(text, systemImage: icon)
            .font(.subheadline)
            .foregroundStyle(.secondary)
    }

    private func load() {
        item = try? library.item(id: itemID)
    }

    /// Applies a triage action and updates the view with the returned item.
    private func perform(_ action: TriageAction) {
        do {
            item = try action.apply(to: itemID, using: library)
        } catch {
            print("[reader] \(action.label) failed for \(itemID): \(error)")
        }
    }
}

#Preview {
    let library = Library()
    return NavigationStack {
        DetailView(library: library, itemID: library.items().first?.id ?? "")
    }
}
