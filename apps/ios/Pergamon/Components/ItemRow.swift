import SwiftUI
import PergamonKit

/// A single content-item row shared by the Inbox, Saved, and Search lists.
struct ItemRow: View {
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
