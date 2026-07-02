import SwiftUI
import PergamonKit

/// A sheet for capturing a new highlight from the reader, or editing the note on
/// an existing one.
///
/// SwiftUI's plain `Text` with `.textSelection` does not hand the selected
/// string back to the app, so capture is a small composer: the user pastes or
/// types the quote and an optional note. Both paths go through the Rust
/// `Library`, which records the highlight (and, on capture, auto-creates a
/// spaced-repetition card) synchronously (ADR-019) and returns the result via
/// `onSave` so the reader refreshes in place.
struct HighlightComposer: View {
    /// What the composer is doing: capturing a new highlight for an item, or
    /// editing the note on an existing highlight.
    enum Mode {
        case capture(itemID: String)
        case editNote(highlight: Highlight)
    }

    let library: Library
    let mode: Mode
    let onSave: () -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var quote: String
    @State private var note: String
    @State private var errorMessage: String?

    init(library: Library, mode: Mode, onSave: @escaping () -> Void) {
        self.library = library
        self.mode = mode
        self.onSave = onSave
        switch mode {
        case .capture:
            _quote = State(initialValue: "")
            _note = State(initialValue: "")
        case .editNote(let highlight):
            _quote = State(initialValue: highlight.quoteText)
            _note = State(initialValue: highlight.note ?? "")
        }
    }

    private var isEditingNote: Bool {
        if case .editNote = mode { return true }
        return false
    }

    private var canSave: Bool {
        isEditingNote || !quote.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Quote") {
                    if isEditingNote {
                        Text(quote)
                            .font(.body)
                            .foregroundStyle(.secondary)
                    } else {
                        TextEditor(text: $quote)
                            .frame(minHeight: 120)
                            .overlay(alignment: .topLeading) {
                                if quote.isEmpty {
                                    Text("Paste or type the passage to highlight")
                                        .foregroundStyle(.tertiary)
                                        .padding(.top, 8)
                                        .allowsHitTesting(false)
                                }
                            }
                    }
                }

                Section("Note (optional)") {
                    TextField("Add a note", text: $note, axis: .vertical)
                        .lineLimit(1...4)
                }

                if !isEditingNote {
                    Section {
                        Label(
                            "Captured highlights become spaced-repetition cards in Review.",
                            systemImage: "brain.head.profile"
                        )
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                    }
                }

                if let errorMessage {
                    Section {
                        Label(errorMessage, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                            .font(.footnote)
                    }
                }
            }
            .navigationTitle(isEditingNote ? "Edit Note" : "New Highlight")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: save)
                        .disabled(!canSave)
                }
            }
        }
    }

    private func save() {
        let trimmedNote = note.trimmingCharacters(in: .whitespacesAndNewlines)
        let noteArg: String? = trimmedNote.isEmpty ? nil : trimmedNote
        do {
            switch mode {
            case .capture(let itemID):
                let trimmedQuote = quote.trimmingCharacters(in: .whitespacesAndNewlines)
                _ = try library.addHighlight(itemId: itemID, quoteText: trimmedQuote, note: noteArg)
            case .editNote(let highlight):
                _ = try library.setHighlightNote(highlightId: highlight.id, note: noteArg)
            }
            onSave()
            dismiss()
        } catch {
            errorMessage = "Couldn't save: \(error)"
        }
    }
}
