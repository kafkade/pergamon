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
    @State private var showingOrganize = false
    @State private var highlights: [Highlight] = []
    @State private var showingHighlightComposer = false
    @State private var editingHighlight: Highlight?

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
                    Button {
                        showingHighlightComposer = true
                    } label: {
                        Label("Highlight", systemImage: "highlighter")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showingOrganize = true
                    } label: {
                        Label("Organize", systemImage: "tag")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    actionsMenu(for: item)
                }
            }
        }
        .sheet(isPresented: $showingOrganize) {
            if let item {
                OrganizeSheet(library: library, item: item) { updated in
                    self.item = updated
                }
            }
        }
        .sheet(isPresented: $showingHighlightComposer) {
            HighlightComposer(library: library, mode: .capture(itemID: itemID)) {
                highlightsChanged()
            }
        }
        .sheet(item: $editingHighlight) { highlight in
            HighlightComposer(library: library, mode: .editNote(highlight: highlight)) {
                highlightsChanged()
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

                organization(for: item)

                Divider()

                articleBody(for: item)

                highlightsSection

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

    /// A compact display of the item's tags and collection memberships, with a
    /// tap target to open the organize sheet. Hidden entirely when the item has
    /// neither, to keep the reader clean.
    @ViewBuilder
    private func organization(for item: ContentItem) -> some View {
        let collectionNames = collectionNames(for: item)
        if !item.tags.isEmpty || !collectionNames.isEmpty {
            Divider()
            Button {
                showingOrganize = true
            } label: {
                VStack(alignment: .leading, spacing: 8) {
                    if !item.tags.isEmpty {
                        WrapHStack(item.tags) { tag in
                            Text("#\(tag)")
                                .font(.caption)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 4)
                                .background(Color.accentColor.opacity(0.12), in: Capsule())
                                .foregroundStyle(Color.accentColor)
                        }
                    }
                    if !collectionNames.isEmpty {
                        Label(collectionNames.joined(separator: " · "), systemImage: Collection.systemImage)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
        }
    }

    /// Resolves the item's collection ids to display names via the library.
    private func collectionNames(for item: ContentItem) -> [String] {
        let membership = Set(item.collectionIds)
        return library.collections()
            .filter { membership.contains($0.id) }
            .map(\.name)
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

    /// The captured highlights for this item: a quote, an optional note, and
    /// per-row actions to edit the note or delete the highlight. Tapping the
    /// header's plus, or the reader toolbar's highlighter, opens the composer.
    /// Hidden when the item has no highlights yet (the toolbar is the entry
    /// point in that case).
    @ViewBuilder
    private var highlightsSection: some View {
        if !highlights.isEmpty {
            Divider()
            VStack(alignment: .leading, spacing: 12) {
                Label("Highlights", systemImage: "highlighter")
                    .font(.headline)

                ForEach(highlights) { highlight in
                    VStack(alignment: .leading, spacing: 6) {
                        Text(highlight.quoteText)
                            .font(.callout)
                            .padding(.leading, 10)
                            .overlay(alignment: .leading) {
                                Rectangle()
                                    .fill(Color.yellow)
                                    .frame(width: 3)
                            }
                        if let note = highlight.note {
                            Label(note, systemImage: "note.text")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        }
                        HStack(spacing: 16) {
                            Button {
                                editingHighlight = highlight
                            } label: {
                                Label(highlight.note == nil ? "Add note" : "Edit note",
                                      systemImage: "square.and.pencil")
                            }
                            Button(role: .destructive) {
                                deleteHighlight(highlight)
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                        }
                        .font(.caption)
                        .buttonStyle(.borderless)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
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
        loadHighlights()
    }

    /// Reloads the captured highlights for this item from the core.
    private func loadHighlights() {
        highlights = (try? library.highlights(itemId: itemID)) ?? []
    }

    /// Reloads highlights and signals that review state changed, so the Review
    /// tab's due-count badge refreshes (capturing a highlight adds a card).
    private func highlightsChanged() {
        loadHighlights()
        NotificationCenter.default.post(name: .pergamonReviewStateChanged, object: nil)
    }

    /// Removes a highlight (and its review card) from the core, then refreshes.
    private func deleteHighlight(_ highlight: Highlight) {
        do {
            try library.deleteHighlight(highlightId: highlight.id)
        } catch {
            print("[reader] delete highlight failed for \(highlight.id): \(error)")
        }
        highlightsChanged()
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
