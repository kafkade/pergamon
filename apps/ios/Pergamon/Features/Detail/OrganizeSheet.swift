import SwiftUI
import PergamonKit

/// A sheet for organizing a single item: assigning and removing tags and
/// collection memberships. Backed by the Rust `Library`, which mutates the local
/// in-memory corpus synchronously (ADR-019) and returns the updated item; each
/// change is reported back through `onUpdate` so the reader stays in sync.
///
/// Users can assign existing tags/collections and create brand-new ones inline
/// (free-form tag names; a name prompt for collections).
struct OrganizeSheet: View {
    let library: Library
    let initialItem: ContentItem
    let onUpdate: (ContentItem) -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var item: ContentItem
    @State private var newTag: String = ""
    @State private var showingNewCollection = false
    @State private var newCollectionName: String = ""
    @State private var errorMessage: String?

    init(library: Library, item: ContentItem, onUpdate: @escaping (ContentItem) -> Void) {
        self.library = library
        self.initialItem = item
        self.onUpdate = onUpdate
        _item = State(initialValue: item)
    }

    var body: some View {
        NavigationStack {
            Form {
                tagsSection
                collectionsSection
                if let errorMessage {
                    Section {
                        Label(errorMessage, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                            .font(.footnote)
                    }
                }
            }
            .navigationTitle("Organize")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .alert("New Collection", isPresented: $showingNewCollection) {
                TextField("Name", text: $newCollectionName)
                Button("Cancel", role: .cancel) { newCollectionName = "" }
                Button("Create") { createCollection() }
            } message: {
                Text("Create a collection and add this item to it.")
            }
        }
    }

    // MARK: - Tags

    private var tagsSection: some View {
        Section("Tags") {
            if !item.tags.isEmpty {
                WrapHStack(item.tags) { tag in
                    Button {
                        removeTag(tag)
                    } label: {
                        HStack(spacing: 4) {
                            Text("#\(tag)")
                            Image(systemName: "xmark.circle.fill")
                        }
                        .font(.caption)
                    }
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(Color.accentColor.opacity(0.12), in: Capsule())
                    .foregroundStyle(Color.accentColor)
                    .accessibilityLabel("Remove tag \(tag)")
                }
            }

            HStack {
                Image(systemName: Tag.systemImage)
                    .foregroundStyle(.secondary)
                TextField("Add a tag", text: $newTag)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .onSubmit(addTypedTag)
                Button("Add", action: addTypedTag)
                    .disabled(newTag.trimmingCharacters(in: .whitespaces).isEmpty)
            }

            let suggestions = suggestedTags
            if !suggestions.isEmpty {
                WrapHStack(suggestions) { name in
                    Button {
                        addTag(name)
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "plus")
                            Text("#\(name)")
                        }
                        .font(.caption)
                    }
                    .buttonStyle(.plain)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(Color.secondary.opacity(0.12), in: Capsule())
                    .foregroundStyle(.secondary)
                }
            }
        }
    }

    /// Existing tags in the library not already on this item.
    private var suggestedTags: [String] {
        let assigned = Set(item.tags.map { $0.lowercased() })
        return library.tags()
            .map(\.name)
            .filter { !assigned.contains($0.lowercased()) }
    }

    private func addTypedTag() {
        let trimmed = newTag.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        addTag(trimmed)
        newTag = ""
    }

    private func addTag(_ name: String) {
        apply { try library.addTag(id: item.id, name: name) }
    }

    private func removeTag(_ name: String) {
        apply { try library.removeTag(id: item.id, name: name) }
    }

    // MARK: - Collections

    private var collectionsSection: some View {
        Section("Collections") {
            ForEach(library.collections()) { collection in
                Button {
                    toggleCollection(collection)
                } label: {
                    HStack {
                        Label(collection.name, systemImage: Collection.systemImage)
                            .padding(.leading, CGFloat(collection.depth) * 16)
                        Spacer()
                        if item.collectionIds.contains(collection.id) {
                            Image(systemName: "checkmark")
                                .foregroundStyle(Color.accentColor)
                        }
                    }
                }
                .tint(.primary)
            }

            Button {
                newCollectionName = ""
                showingNewCollection = true
            } label: {
                Label("New Collection", systemImage: "folder.badge.plus")
            }
        }
    }

    private func toggleCollection(_ collection: Collection) {
        if item.collectionIds.contains(collection.id) {
            apply { try library.removeFromCollection(id: item.id, collectionId: collection.id) }
        } else {
            apply { try library.addToCollection(id: item.id, collectionId: collection.id) }
        }
    }

    private func createCollection() {
        let name = newCollectionName.trimmingCharacters(in: .whitespaces)
        newCollectionName = ""
        guard !name.isEmpty else { return }
        do {
            let created = try library.createCollection(name: name, parentId: nil)
            apply { try library.addToCollection(id: item.id, collectionId: created.id) }
        } catch {
            errorMessage = "\(error)"
        }
    }

    // MARK: - Mutation plumbing

    /// Runs a library mutation, updates the local copy, and reports the change
    /// upward. Surfaces any error inline rather than crashing.
    private func apply(_ mutation: () throws -> ContentItem) {
        do {
            let updated = try mutation()
            item = updated
            errorMessage = nil
            onUpdate(updated)
        } catch {
            errorMessage = "\(error)"
        }
    }
}
