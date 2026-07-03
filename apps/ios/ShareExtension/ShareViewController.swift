import UIKit
import UniformTypeIdentifiers

/// The pergamon **share extension** — the entry point when a user taps
/// *pergamon* in the iOS share sheet from Safari or any app.
///
/// Per **ADR-021**, this does the *minimum work to capture and hand off*: it
/// pulls a URL, a text selection, and a best-effort title out of the share
/// sheet's item providers, writes exactly one atomic JSON record to the shared
/// staging drop folder, and returns. It performs **no** network fetch, **no**
/// extraction, and **no** database access — all of that is deferred to the main
/// app, which finalizes the capture through the Rust core. Staying this light is
/// what keeps the extension inside iOS's tight memory/time budget so a save
/// completes in well under five seconds.
final class ShareViewController: UIViewController {
    private let statusLabel = UILabel()
    private let activity = UIActivityIndicatorView(style: .medium)

    override func viewDidLoad() {
        super.viewDidLoad()
        configureConfirmationUI()

        Task {
            await handleShare()
        }
    }

    // MARK: - Capture

    private func handleShare() async {
        let capture = await extractCapture()

        guard let capture else {
            await finish(message: "Nothing to save", success: false)
            return
        }

        guard let inbox = StagingInbox.shared() else {
            await finish(message: "Sharing unavailable", success: false)
            return
        }

        do {
            try inbox.write(capture)
            await finish(message: "Saved to pergamon", success: true)
        } catch {
            await finish(message: "Couldn't save", success: false)
        }
    }

    /// Builds a ``StagedCapture`` from the extension's item providers, or `nil`
    /// when the shared content carries neither a URL nor text.
    private func extractCapture() async -> StagedCapture? {
        let items = (extensionContext?.inputItems as? [NSExtensionItem]) ?? []

        var url: String?
        var text: String?
        var pageTitle: String?

        for item in items {
            // The share sheet's own title/summary is a best-effort page title we
            // can record without a fetch.
            if pageTitle == nil, let title = item.attributedTitle?.string, !title.isEmpty {
                pageTitle = title
            }
            let contentText = item.attributedContentText?.string

            for provider in item.attachments ?? [] {
                if url == nil, let loaded = await loadURL(from: provider) {
                    url = loaded
                } else if text == nil, let loaded = await loadText(from: provider) {
                    text = loaded
                }
            }

            // A URL share often surfaces the page title as the content text; a
            // plain-text share surfaces the selection there instead.
            if let contentText, !contentText.isEmpty {
                if url != nil, text == nil {
                    pageTitle = pageTitle ?? contentText
                } else if url == nil, text == nil {
                    text = contentText
                }
            }
        }

        return makeCapture(url: url, text: text, pageTitle: pageTitle)
    }

    /// Chooses the `content_kind` from what was actually captured, trimming empty
    /// values, and stamps the originating bundle id for provenance.
    private func makeCapture(url: String?, text: String?, pageTitle: String?) -> StagedCapture? {
        let url = url?.trimmingCharacters(in: .whitespacesAndNewlines)
        let text = text?.trimmingCharacters(in: .whitespacesAndNewlines)
        let title = pageTitle?.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanURL = (url?.isEmpty == false) ? url : nil
        let cleanText = (text?.isEmpty == false) ? text : nil
        let cleanTitle = (title?.isEmpty == false) ? title : nil
        let source = Bundle.main.bundleIdentifier

        switch (cleanURL, cleanText) {
        case let (someURL?, someText?):
            return StagedCapture(
                contentKind: .urlWithSelection,
                url: someURL,
                selectedText: someText,
                pageTitle: cleanTitle,
                sourceApp: source
            )
        case let (someURL?, nil):
            return StagedCapture(
                contentKind: .url,
                url: someURL,
                pageTitle: cleanTitle,
                sourceApp: source
            )
        case let (nil, someText?):
            return StagedCapture(
                contentKind: .text,
                selectedText: someText,
                pageTitle: cleanTitle,
                sourceApp: source
            )
        case (nil, nil):
            return nil
        }
    }

    // MARK: - Provider loading

    private func loadURL(from provider: NSItemProvider) async -> String? {
        guard provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) else { return nil }
        return await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: UTType.url.identifier, options: nil) { value, _ in
                if let pageURL = value as? URL {
                    continuation.resume(returning: pageURL.absoluteString)
                } else if let string = value as? String {
                    continuation.resume(returning: string)
                } else {
                    continuation.resume(returning: nil)
                }
            }
        }
    }

    private func loadText(from provider: NSItemProvider) async -> String? {
        let identifier = UTType.plainText.identifier
        guard provider.hasItemConformingToTypeIdentifier(identifier) else { return nil }
        return await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: identifier, options: nil) { value, _ in
                if let string = value as? String {
                    continuation.resume(returning: string)
                } else if let data = value as? Data {
                    continuation.resume(returning: String(data: data, encoding: .utf8))
                } else {
                    continuation.resume(returning: nil)
                }
            }
        }
    }

    // MARK: - UI + completion

    private func configureConfirmationUI() {
        view.backgroundColor = .clear
        let panel = UIView()
        panel.translatesAutoresizingMaskIntoConstraints = false
        panel.backgroundColor = .secondarySystemBackground
        panel.layer.cornerRadius = 14
        view.addSubview(panel)

        statusLabel.text = "Saving…"
        statusLabel.font = .preferredFont(forTextStyle: .headline)
        statusLabel.textColor = .label
        statusLabel.translatesAutoresizingMaskIntoConstraints = false

        activity.translatesAutoresizingMaskIntoConstraints = false
        activity.startAnimating()

        panel.addSubview(activity)
        panel.addSubview(statusLabel)

        NSLayoutConstraint.activate([
            panel.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            panel.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            panel.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 40),

            activity.leadingAnchor.constraint(equalTo: panel.leadingAnchor, constant: 20),
            activity.centerYAnchor.constraint(equalTo: panel.centerYAnchor),

            statusLabel.leadingAnchor.constraint(equalTo: activity.trailingAnchor, constant: 12),
            statusLabel.trailingAnchor.constraint(equalTo: panel.trailingAnchor, constant: -20),
            statusLabel.topAnchor.constraint(equalTo: panel.topAnchor, constant: 18),
            statusLabel.bottomAnchor.constraint(equalTo: panel.bottomAnchor, constant: -18),
        ])
    }

    /// Shows a brief confirmation, then completes the extension request. Runs on
    /// the main actor because it touches UIKit.
    @MainActor
    private func finish(message: String, success: Bool) async {
        activity.stopAnimating()
        activity.isHidden = true
        statusLabel.text = message

        // A short beat so the user sees the confirmation, then dismiss.
        try? await Task.sleep(nanoseconds: 550_000_000)

        if success {
            extensionContext?.completeRequest(returningItems: [], completionHandler: nil)
        } else {
            let error = NSError(domain: "dev.pergamon.share", code: 1)
            extensionContext?.cancelRequest(withError: error)
        }
    }
}
