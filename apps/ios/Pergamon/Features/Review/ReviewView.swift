import SwiftUI

/// The Review tab: spaced-repetition review of highlights and notes.
///
/// The FSRS engine lives in `pergamon-core`, but the UniFFI facade
/// (`pergamon-uniffi`) does not yet export a review surface, so this scaffolds
/// the tab with a placeholder. It is wired into the navigation shell now so the
/// review work can fill it in without touching the app structure.
struct ReviewView: View {
    var body: some View {
        NavigationStack {
            ContentUnavailableView(
                "No cards due",
                systemImage: "brain.head.profile",
                description: Text("Spaced-repetition review arrives with the review surface. Highlights you capture will become cards here.")
            )
            .navigationTitle("Review")
        }
    }
}

#Preview {
    ReviewView()
}
