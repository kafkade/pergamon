import SwiftUI

/// Detail / reader view. Demonstrates the "open" path: rather than passing the
/// already-loaded struct, it re-fetches the item from Rust by id via
/// `getItem(id:)`, proving a round-trip lookup across the FFI boundary.
struct DetailView: View {
    let itemID: String

    private var item: ContentItem? { getItem(id: itemID) }

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
        .navigationTitle("Item")
        .navigationBarTitleDisplayMode(.inline)
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
                }
                .font(.caption)

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

                if let excerpt = item.excerpt {
                    Divider()
                    Text(excerpt)
                        .font(.body)
                        .foregroundStyle(.primary)
                }

                Divider()
                Text("id \(item.id)")
                    .font(.caption2.monospaced())
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()
        }
    }

    private func metadata(icon: String, text: String) -> some View {
        Label(text, systemImage: icon)
            .font(.subheadline)
            .foregroundStyle(.secondary)
    }
}

#Preview {
    NavigationStack {
        DetailView(itemID: sampleItems().first?.id ?? "")
    }
}
