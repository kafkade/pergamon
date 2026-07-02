import SwiftUI
import PergamonKit

/// The inbox tab: newly-captured items awaiting triage (`library.inbox()`), the
/// app's primary landing screen. Tapping a row opens it in `DetailView` via a
/// fresh `library.item(id:)` round-trip into Rust.
struct InboxView: View {
    @EnvironmentObject private var environment: AppEnvironment

    private var items: [ContentItem] { environment.library.inbox() }

    var body: some View {
        NavigationStack {
            Group {
                if items.isEmpty {
                    ContentUnavailableView(
                        "Inbox zero",
                        systemImage: "tray",
                        description: Text("Nothing to triage. Saved items land here first.")
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
            .navigationTitle("Inbox")
            .navigationDestination(for: ContentItem.self) { item in
                DetailView(library: environment.library, itemID: item.id)
            }
        }
    }
}

#Preview {
    InboxView()
        .environmentObject(AppEnvironment())
}
