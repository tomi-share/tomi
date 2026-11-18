import Foundation
import TomiKit
import UIKit
import UniformTypeIdentifiers

/// What the extension received, read from the system types (Android `ShareIntentReader`): the text, link, HTML and title
/// of the share, and its files copied into the inbox (the system lends them only for the moment of the reading), within
/// the limits of an item (25 MB a file, 100 MB in all, 20 files).
struct ShareItemReader {
    struct Result {
        var input: ShareInput
        /// Files left out: unreadable, or over a limit.
        var skipped: Int
    }

    let directory: URL

    func read(_ items: [NSExtensionItem]) async -> Result {
        var state = State()
        for item in items {
            if state.subject == nil, let title = item.attributedTitle?.string, !Self.isBlank(title) { state.subject = title }
            for provider in item.attachments ?? [] {
                await read(provider, into: &state)
            }
            // Some apps put their text in the item rather than in an attachment.
            if state.texts.isEmpty, let content = item.attributedContentText?.string, !Self.isBlank(content) {
                state.texts.append(content)
            }
        }
        // "Title URL", as Safari and most apps share it: the parser splits it again.
        let urls = state.urls.filter { url in !state.texts.contains { $0.contains(url) } }
        let text = (state.texts + urls).joined(separator: "\n")
        return Result(
            input: ShareInput(subject: state.subject, text: text.isEmpty ? nil : text, htmlText: state.html, files: state.files),
            skipped: state.skipped
        )
    }

    private struct State {
        var subject: String?
        var texts: [String] = []
        var urls: [String] = []
        var html: String?
        var files: [SharedFile] = []
        var total: Int64 = 0
        var skipped = 0
    }

    private func read(_ provider: NSItemProvider, into state: inout State) async {
        // A file of the Files app, or a document.
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            if let url = await load(provider, .fileURL) as? URL { add(url, name: url.lastPathComponent, to: &state) }
            return
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            if let url = await load(provider, .url) as? URL {
                if url.isFileURL { add(url, name: url.lastPathComponent, to: &state) } else { state.urls.append(url.absoluteString) }
            }
            return
        }
        // Text: the plain version and the HTML one when both are given (Notes, Mail), any other text type as text.
        let isText = provider.registeredTypeIdentifiers.contains { UTType($0)?.conforms(to: .text) == true }
        if isText {
            if provider.hasItemConformingToTypeIdentifier(UTType.html.identifier), state.html == nil {
                switch await load(provider, .html) {
                case let value as String: state.html = value
                case let data as Data: state.html = String(data: data, encoding: .utf8)
                default: break
                }
            }
            if let text = await plainText(provider) { state.texts.append(text) }
            return
        }
        // A file given by its contents (Photos, a screenshot): its type, data first, never a package.
        guard let type = Self.fileType(provider) else { return }
        guard state.files.count < ShareLimits.maxFiles else {
            state.skipped += 1
            return
        }
        let name = provider.suggestedName
        let lent: (URL, String)? = await withCheckedContinuation { continuation in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, _ in
                guard let url else { return continuation.resume(returning: nil) }
                // Copied before returning: the system deletes its file then.
                let target = self.directory.appendingPathComponent("lent-\(UUID().uuidString)")
                guard (try? FileManager.default.copyItem(at: url, to: target)) != nil else { return continuation.resume(returning: nil) }
                continuation.resume(returning: (target, Self.name(suggested: name, url: url)))
            }
        }
        guard let (url, fullName) = lent else {
            state.skipped += 1
            return
        }
        add(url, name: fullName, to: &state, owned: true)
    }

    /// The plain text of `provider`, nil when it has none.
    private func plainText(_ provider: NSItemProvider) async -> String? {
        let types: [UTType] = [.plainText, .utf8PlainText, .text]
        for type in types where provider.hasItemConformingToTypeIdentifier(type.identifier) {
            switch await load(provider, type) {
            case let value as String: return value
            case let value as NSAttributedString: return value.string
            case let data as Data:
                if let rich = try? NSAttributedString(data: data, options: [:], documentAttributes: nil) { return rich.string }
                if let value = String(data: data, encoding: .utf8) { return value }
            default: continue
            }
        }
        return nil
    }

    /// Keeps the file at `source` if it fits the limits: copied into the directory of the reading (or kept there when
    /// `owned`, already copied).
    private func add(_ source: URL, name: String, to state: inout State, owned: Bool = false) {
        defer { if owned, !state.files.contains(where: { $0.url == source }) { try? FileManager.default.removeItem(at: source) } }
        guard state.files.count < ShareLimits.maxFiles else {
            state.skipped += 1
            return
        }
        let scoped = !owned && source.startAccessingSecurityScopedResource()
        defer { if scoped { source.stopAccessingSecurityScopedResource() } }
        let size = (try? source.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? -1
        guard size >= 0, size <= ShareLimits.maxFileBytes, state.total + size <= ShareLimits.maxItemBytes else {
            state.skipped += 1
            return
        }
        var target = source
        if !owned {
            target = directory.appendingPathComponent("\(state.files.count)-\(UUID().uuidString)")
            guard (try? FileManager.default.copyItem(at: source, to: target)) != nil else {
                state.skipped += 1
                return
            }
        }
        state.total += size
        let mime = UTType(filenameExtension: (name as NSString).pathExtension)?.preferredMIMEType ?? "application/octet-stream"
        state.files.append(SharedFile(url: target, name: name, mime: mime, size: size))
    }

    /// The type to ask a file in: a data type (an image, a movie, a PDF...), never a package (a Live Photo bundle has
    /// no size).
    private static func fileType(_ provider: NSItemProvider) -> UTType? {
        let types = provider.registeredTypeIdentifiers.compactMap(UTType.init)
        return types.first { $0.conforms(to: .data) && !$0.conforms(to: .package) }
            ?? types.first { ($0.conforms(to: .image) || $0.conforms(to: .movie)) && !$0.conforms(to: .package) }
    }

    /// The suggested name keeps the extension of the file the system lent.
    private static func name(suggested: String?, url: URL) -> String {
        guard let suggested, !isBlank(suggested) else { return url.lastPathComponent }
        let ext = url.pathExtension
        return ext.isEmpty || (suggested as NSString).pathExtension.lowercased() == ext.lowercased() ? suggested : "\(suggested).\(ext)"
    }

    private static func isBlank(_ text: String) -> Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func load(_ provider: NSItemProvider, _ type: UTType) async -> NSSecureCoding? {
        await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: type.identifier, options: nil) { value, _ in
                continuation.resume(returning: value)
            }
        }
    }
}
