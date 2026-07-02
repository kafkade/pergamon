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
                    .fontWeight(item.isRead ? .regular : .semibold)
                    .foregroundStyle(item.isRead ? .secondary : .primary)
                    .lineLimit(2)

                if let source = item.sourceName {
                    Text(source)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                } else if let author = item.author {
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

            Spacer(minLength: 0)

            if !item.isRead {
                Circle()
                    .fill(Color.accentColor)
                    .frame(width: 8, height: 8)
                    .padding(.top, 6)
                    .accessibilityLabel("Unread")
            }
        }
        .padding(.vertical, 4)
    }
}
