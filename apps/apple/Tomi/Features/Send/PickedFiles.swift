import CoreTransferable
import Foundation
import TomiKit
import UniformTypeIdentifiers

/// Copies of the files chosen on the send screen, in `CorePaths.picked/<id>/<name>`. The system lends a file from
/// Files (security-scoped) or Photos only for the moment of the choice: it is copied at once, then handed to the send
/// as a plain path. A copy goes when the file is removed, sent (after the undo window), or at the next start.
final class PickedFiles: Sendable {
    let root: URL

    init(root: URL) {
        self.root = root
    }

    /// Erases every copy left by a previous run (nothing chosen survives a restart).
    func wipe() {
        try? FileManager.default.removeItem(at: root)
    }

    func delete(_ file: SharedFile) {
        let parent = file.url.deletingLastPathComponent()
        guard parent.deletingLastPathComponent().standardizedFileURL == root.standardizedFileURL else { return }
        try? FileManager.default.removeItem(at: parent)
    }

    /// A file from Files (`fileImporter`). Nil when it is unreadable or over 25 MB.
    func importFile(_ url: URL) async -> SharedFile? {
        let root = self.root
        return await Task.detached(priority: .userInitiated) {
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            // A coordinated read downloads a file kept in iCloud first, and hands a stable copy.
            var copied: SharedFile?
            var error: NSError?
            NSFileCoordinator().coordinate(readingItemAt: url, options: .withoutChanges, error: &error) { readable in
                copied = Self.copy(from: readable, name: url.lastPathComponent, root: root)
            }
            return copied
        }.value
    }

    /// A file the photo picker wrote to a temporary place (`PickedMedia`).
    func importMedia(_ media: PickedMedia) async -> SharedFile? {
        let root = self.root
        return await Task.detached(priority: .userInitiated) {
            defer { try? FileManager.default.removeItem(at: media.url.deletingLastPathComponent()) }
            return Self.copy(from: media.url, name: media.url.lastPathComponent, root: root)
        }.value
    }

    private static func copy(from source: URL, name: String, root: URL) -> SharedFile? {
        let size = (try? source.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? -1
        guard size >= 0, size <= ShareLimits.maxFileBytes else { return nil }
        let dir = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let target = dir.appendingPathComponent(name.isEmpty ? "file" : name)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: source, to: target)
        } catch {
            try? FileManager.default.removeItem(at: dir)
            return nil
        }
        let mime = UTType(filenameExtension: target.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
        return SharedFile(url: target, name: target.lastPathComponent, mime: mime, size: size)
    }
}

/// A photo or a video from the photo picker, received as a file (its own name and format kept).
struct PickedMedia: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .image) { received in try PickedMedia(received.file) }
        FileRepresentation(importedContentType: .movie) { received in try PickedMedia(received.file) }
    }

    /// The received file is only valid during the call: moved into a directory of its own.
    private init(_ file: URL) throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let target = dir.appendingPathComponent(file.lastPathComponent)
        try FileManager.default.copyItem(at: file, to: target)
        url = target
    }
}
