import SwiftUI
import PergamonKit

/// The Review tab: a spaced-repetition queue over the highlights you've
/// captured, scheduled by the FSRS engine in `pergamon-core` (via the UniFFI
/// facade). Everything runs against the local core, so review works fully
/// offline and schedules identically to the CLI.
///
/// The session loads the cards that are due (`dueCards`), shows one at a time —
/// the highlight quote as the prompt, then the note/source revealed as the
/// answer — and grades it with the four FSRS buttons. Grading writes a review
/// log and reschedules the card in the core. An "Again" grade re-enqueues the
/// card later in the session, mirroring the CLI review loop.
struct ReviewView: View {
    @EnvironmentObject private var environment: AppEnvironment

    @State private var queue: [ReviewCardView] = []
    @State private var index = 0
    @State private var showingAnswer = false
    @State private var reviewedCount = 0

    private var library: Library { environment.library }

    private var current: ReviewCardView? {
        index < queue.count ? queue[index] : nil
    }

    var body: some View {
        NavigationStack {
            Group {
                if let card = current {
                    reviewCard(card)
                } else if reviewedCount > 0 {
                    finishedState
                } else {
                    emptyState
                }
            }
            .navigationTitle("Review")
            .toolbar {
                if !queue.isEmpty, current != nil {
                    ToolbarItem(placement: .topBarTrailing) {
                        Text("\(index + 1) of \(queue.count)")
                            .font(.subheadline.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
        .onAppear(perform: loadQueue)
    }

    // MARK: - Card

    private func reviewCard(_ card: ReviewCardView) -> some View {
        VStack(spacing: 20) {
            HStack {
                Label(card.state.label, systemImage: "circle.fill")
                    .font(.caption2)
                    .foregroundStyle(card.state.tint)
                Spacer()
                Label(card.sourceTitle, systemImage: "doc.text")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer()

            VStack(spacing: 16) {
                Text(card.quoteText)
                    .font(.title3)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)

                if showingAnswer {
                    Divider()
                    if let note = card.note {
                        Label(note, systemImage: "note.text")
                            .font(.body)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    } else {
                        Text("No note on this highlight.")
                            .font(.callout)
                            .foregroundStyle(.tertiary)
                    }
                }
            }
            .frame(maxWidth: .infinity)

            Spacer()

            if showingAnswer {
                gradeButtons(for: card)
            } else {
                Button {
                    withAnimation { showingAnswer = true }
                } label: {
                    Label("Show Answer", systemImage: "eye")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            }
        }
        .padding()
    }

    private func gradeButtons(for card: ReviewCardView) -> some View {
        HStack(spacing: 10) {
            ForEach(grades, id: \.self) { option in
                Button {
                    grade(option, card: card)
                } label: {
                    VStack(spacing: 4) {
                        Image(systemName: option.systemImage)
                        Text(option.label)
                            .font(.caption)
                    }
                    .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
                .tint(option.tint)
            }
        }
    }

    private var grades: [ReviewGrade] { [.again, .hard, .good, .easy] }

    // MARK: - Empty / finished states

    private var emptyState: some View {
        ContentUnavailableView(
            "No cards due",
            systemImage: "checkmark.circle",
            description: Text("Highlights you capture in the reader become review cards and appear here when they're due.")
        )
    }

    private var finishedState: some View {
        ContentUnavailableView {
            Label("All caught up", systemImage: "sparkles")
        } description: {
            Text("You reviewed \(reviewedCount) card\(reviewedCount == 1 ? "" : "s") this session.")
        } actions: {
            Button("Check for more") { loadQueue() }
                .buttonStyle(.borderedProminent)
        }
    }

    // MARK: - Actions

    private func loadQueue() {
        queue = library.dueCards()
        index = 0
        reviewedCount = 0
        showingAnswer = false
    }

    /// Grades the current card, advancing the FSRS schedule in the core. On
    /// "Again" the (rescheduled) card is re-enqueued at the end of the session
    /// so the user keeps practicing it, matching the CLI review loop.
    private func grade(_ option: ReviewGrade, card: ReviewCardView) {
        do {
            let updated = try library.gradeCard(cardId: card.cardId, grade: option)
            if option == .again {
                queue.append(updated)
            }
        } catch {
            print("[review] grade \(option.label) failed for \(card.cardId): \(error)")
        }
        reviewedCount += 1
        index += 1
        showingAnswer = false
        NotificationCenter.default.post(name: .pergamonReviewStateChanged, object: nil)
    }
}

#Preview {
    ReviewView()
        .environmentObject(AppEnvironment())
}
