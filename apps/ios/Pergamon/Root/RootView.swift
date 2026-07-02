import SwiftUI

/// The app's top-level navigation shell: a tab bar over the four primary
/// surfaces. Each tab owns its own `NavigationStack` so navigation state is
/// independent per tab. All tabs read the shared `Library` from the injected
/// `AppEnvironment`.
struct RootView: View {
    /// The four primary tabs of the app.
    private enum Tab: Hashable {
        case inbox, saved, search, review
    }

    @State private var selection: Tab = .inbox

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
        }
    }
}

#Preview {
    RootView()
        .environmentObject(AppEnvironment())
}
