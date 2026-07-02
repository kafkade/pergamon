import SwiftUI

/// A layout that arranges its subviews left-to-right, wrapping to the next line
/// when the current line runs out of horizontal space — a "flow" / "tag cloud"
/// layout. Used for tag chips in the reader and organize sheet.
struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        let rows = arrange(subviews: subviews, maxWidth: maxWidth)
        let width = rows.map { $0.width }.max() ?? 0
        let height = rows.reduce(0) { $0 + $1.height } + spacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: min(width, maxWidth), height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) {
        let rows = arrange(subviews: subviews, maxWidth: bounds.width)
        var y = bounds.minY
        for row in rows {
            var x = bounds.minX
            for element in row.elements {
                let size = subviews[element.index].sizeThatFits(.unspecified)
                subviews[element.index].place(
                    at: CGPoint(x: x, y: y),
                    anchor: .topLeading,
                    proposal: ProposedViewSize(size)
                )
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private struct Row {
        var elements: [(index: Int, width: CGFloat)] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func arrange(subviews: Subviews, maxWidth: CGFloat) -> [Row] {
        var rows: [Row] = []
        var current = Row()
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            let projected = current.width == 0 ? size.width : current.width + spacing + size.width
            if projected > maxWidth, !current.elements.isEmpty {
                rows.append(current)
                current = Row()
            }
            current.width = current.width == 0 ? size.width : current.width + spacing + size.width
            current.height = max(current.height, size.height)
            current.elements.append((index, size.width))
        }
        if !current.elements.isEmpty { rows.append(current) }
        return rows
    }
}

/// Convenience wrapper that renders `content` for each item in a wrapping flow.
struct WrapHStack<Content: View>: View {
    private let items: [String]
    private let spacing: CGFloat
    private let content: (String) -> Content

    init(_ items: [String], spacing: CGFloat = 8, @ViewBuilder content: @escaping (String) -> Content) {
        self.items = items
        self.spacing = spacing
        self.content = content
    }

    var body: some View {
        FlowLayout(spacing: spacing) {
            ForEach(items, id: \.self) { item in
                content(item)
            }
        }
    }
}
