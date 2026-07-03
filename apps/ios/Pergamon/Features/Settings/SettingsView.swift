import PergamonKit
import SwiftUI
import UniformTypeIdentifiers

/// Settings and data-management surface. Its primary job for #118 is **backup
/// import/export**: the whole library round-trips through the canonical ZIP
/// archive format shared with the CLI and web clients, so a backup taken on any
/// pergamon client restores here and vice versa.
///
/// - Export writes the archive to a temporary file and hands it to the system
///   share sheet, so the user can save it to Files, iCloud Drive, or AirDrop.
/// - Restore picks a `.zip` archive with the document picker and **replaces**
///   the current library, then broadcasts `.pergamonLibraryDidChange` so every
///   list reloads.
struct SettingsView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @Environment(\.dismiss) private var dismiss

    @State private var info: StorageInfo?
    @State private var exportFile: ExportFile?
    @State private var showingImporter = false
    @State private var isRestoring = false
    @State private var banner: Banner?

    private var library: Library { environment.library }

    var body: some View {
        NavigationStack {
            Form {
                storageSection
                backupSection
                aboutSection
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
            .onAppear { info = library.storageInfo() }
            .sheet(item: $exportFile) { file in
                ShareSheet(items: [file.url])
            }
            .fileImporter(
                isPresented: $showingImporter,
                allowedContentTypes: [.zip],
                allowsMultipleSelection: false
            ) { result in
                handleImport(result)
            }
            .alert(item: $banner) { banner in
                Alert(
                    title: Text(banner.title),
                    message: Text(banner.message),
                    dismissButton: .default(Text("OK"))
                )
            }
        }
    }

    // MARK: - Sections

    @ViewBuilder private var storageSection: some View {
        Section("Storage") {
            LabeledContent(
                "Store",
                value: environment.usingPersistentStore ? "On-device SQLite" : "In-memory (fallback)"
            )
            if let info {
                LabeledContent("Documents", value: "\(info.documentCount)")
                LabeledContent("Highlights", value: "\(info.highlightCount)")
                LabeledContent("Schema version", value: "\(info.schemaVersion)")
            }
        }
    }

    @ViewBuilder private var backupSection: some View {
        Section {
            Button {
                exportBackup()
            } label: {
                Label("Export Backup…", systemImage: "square.and.arrow.up")
            }

            Button(role: .destructive) {
                showingImporter = true
            } label: {
                Label("Restore from Backup…", systemImage: "square.and.arrow.down")
            }
            .disabled(isRestoring)
        } header: {
            Text("Backup")
        } footer: {
            Text("Backups use pergamon's portable archive format, shared with the desktop and web apps. Restoring replaces the entire on-device library.")
        }
    }

    @ViewBuilder private var aboutSection: some View {
        Section("About") {
            LabeledContent("pergamon-core", value: environment.coreVersion)
        }
    }

    // MARK: - Actions

    /// Writes a backup archive to a timestamped temp file and presents the share
    /// sheet so the user can save it wherever they like.
    private func exportBackup() {
        let name = "pergamon-backup-\(Self.timestamp()).zip"
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(name)
        do {
            let summary = try library.exportBackup(path: url.path)
            exportFile = ExportFile(url: url)
            print("[settings] exported backup (\(summary.total) records) → \(url.lastPathComponent)")
        } catch {
            banner = Banner(title: "Export Failed", message: readable(error))
        }
    }

    /// Restores the picked archive, replacing the library, then broadcasts the
    /// change so every surface reloads.
    private func handleImport(_ result: Result<[URL], Error>) {
        switch result {
        case let .success(urls):
            guard let url = urls.first else { return }
            restore(from: url)
        case let .failure(error):
            banner = Banner(title: "Restore Failed", message: readable(error))
        }
    }

    private func restore(from url: URL) {
        isRestoring = true
        // Security-scoped access is required for files vended by the document
        // picker from outside the app sandbox (iCloud Drive, other providers).
        let scoped = url.startAccessingSecurityScopedResource()
        defer {
            if scoped { url.stopAccessingSecurityScopedResource() }
            isRestoring = false
        }
        do {
            let summary = try library.restoreBackup(path: url.path)
            info = library.storageInfo()
            NotificationCenter.default.post(name: .pergamonLibraryDidChange, object: nil)
            NotificationCenter.default.post(name: .pergamonReviewStateChanged, object: nil)
            banner = Banner(
                title: "Restore Complete",
                message: "Imported \(summary.total) records: \(summary.contentItems) items, \(summary.feeds) feeds, \(summary.tags) tags, \(summary.collections) collections."
            )
        } catch {
            banner = Banner(title: "Restore Failed", message: readable(error))
        }
    }

    /// Extracts the human-readable message from a `PergamonError`, falling back
    /// to the system description for other error types.
    private func readable(_ error: Error) -> String {
        switch error {
        case let PergamonError.NotFound(message): return message
        case let PergamonError.InvalidInput(message): return message
        case let PergamonError.Storage(message): return message
        case let PergamonError.Network(message): return message
        case let PergamonError.Internal(message): return message
        default: return error.localizedDescription
        }
    }

    private static func timestamp() -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd-HHmmss"
        return formatter.string(from: Date())
    }
}

/// A lightweight identifiable alert payload.
private struct Banner: Identifiable {
    let id = UUID()
    let title: String
    let message: String
}

/// Wraps the exported archive URL so it can drive `.sheet(item:)` without a
/// retroactive `URL: Identifiable` conformance.
private struct ExportFile: Identifiable {
    let url: URL
    var id: String { url.absoluteString }
}

/// Bridges `UIActivityViewController` so a backup file can be shared to Files,
/// iCloud Drive, AirDrop, etc.
private struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

#Preview {
    SettingsView()
        .environmentObject(AppEnvironment())
}
