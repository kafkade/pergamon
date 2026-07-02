import SwiftUI

/// The app's top-level navigation shell: a tab bar over the four primary
/// surfaces. Each tab owns its own `NavigationStack` so navigation state is
/// independent per tab. All tabs read the shared `Library` from the injected
/// `AppEnvironment`.
struct RootView: View {
    @EnvironmentObject private var environment: AppEnvironment

    /// The four primary tabs of the app.
    private enum Tab: Hashable {
        case inbox, saved, search, review
    }

    @State private var selection: Tab = .inbox
    /// Number of review cards currently due, surfaced as a badge on the Review
    /// tab. Recomputed from the core when the selection changes and whenever
    /// review state changes (a highlight captured, a card graded).
    @State private var dueCount = 0

    var body: some View {
        TabView(selection: $selection) {
            InboxView()
                .tabItem { Label("Inbox", systemImage: "tray") }
                .tag(Tab.inbox)

            SavedView()
                .tabItem { Label("Saved", systemImage: "square.stack.3d.up") }
                .tag(Tab.saved)

            SearchView()
                .tabItem { Label("Search", systemImage: "magnifyingglass") }
                .tag(Tab.search)

            ReviewView()
                .tabItem { Label("Review", systemImage: "brain.head.profile") }
                .tag(Tab.review)
                .badge(dueCount)
        }
        .onAppear(perform: refreshDueCount)
        .onChange(of: selection) { refreshDueCount() }
        .onReceive(NotificationCenter.default.publisher(for: .pergamonReviewStateChanged)) { _ in
            refreshDueCount()
        }
    }

    private func refreshDueCount() {
        dueCount = Int(environment.library.reviewSummary().dueCount)
    }
}

#Preview {
    RootView()
        .environmentObject(AppEnvironment())
}
