import Foundation
import ImageIO
import TomiKit
import UniformTypeIdentifiers

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/send/FileAttachmentStaging.kt.

/// Copies the files of a draft to `files/outgoing/<draftId>/` within the limits (25 MB per file, 100 MB per item,
/// 20 files): a file over a limit or unreadable is skipped, the others are kept. Names are cleaned (no path, no control
/// character). Then makes the preview each file carries inside the item (`DraftThumbnails`: images, and the PDF, videos
/// and documents Quick Look can draw): the only place where the app makes thumbnails, as the core expects
/// (`DraftAttachment.thumbnail`). The budget of the item and the rule of hidden items apply when the draft is mapped.
public struct FileAttachmentStaging: AttachmentStaging {
    private static let maxName = 120
    private static let bufferSize = 64 * 1024

    private let root: URL

    public init(root: URL) {
        self.root = root
    }

    public func stage(draftId: String, files: [SharedFile]) async -> StagedAttachments {
        let root = self.root
        return await Task.detached(priority: .userInitiated) {
            Self.stageNow(root: root, draftId: draftId, files: files)
        }.value
    }

    /// The thumbnails of a draft that is not hidden, in order, until the budget of the item is spent (the next files go
    /// without: none is made for nothing).
    public func thumbnails(_ attachments: [DraftAttachment]) async -> [DraftAttachment] {
        var spent = 0
        var result: [DraftAttachment] = []
        for attachment in attachments {
            var withThumbnail = attachment
            if spent < ThumbnailBudget.budgetBytes,
               let thumbnail = await DraftThumbnails.make(
                URL(fileURLWithPath: attachment.path), name: attachment.name, mime: attachment.mime
               ) {
                withThumbnail.thumbnail = thumbnail.data
                withThumbnail.thumbnailMime = thumbnail.mime
                spent += thumbnail.data.count
            }
            result.append(withThumbnail)
        }
        return result
    }

    public func cleanup(draftId: String) {
        try? FileManager.default.removeItem(at: directory(root: root, draftId: draftId))
    }

    static func stageNow(root: URL, draftId: String, files: [SharedFile]) -> StagedAttachments {
        guard !files.isEmpty else { return StagedAttachments(attachments: [], skipped: 0) }
        let dir = directory(root: root, draftId: draftId)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var staged: [DraftAttachment] = []
        var skipped = max(files.count - ShareLimits.maxFiles, 0)
        var total: Int64 = 0
        for (index, file) in files.prefix(ShareLimits.maxFiles).enumerated() {
            if file.size > ShareLimits.maxFileBytes || total + max(file.size, 0) > ShareLimits.maxItemBytes {
                skipped += 1
                continue
            }
            let name = safeName(file.name, index: index)
            let target = dir.appendingPathComponent("\(index)-\(name)")
            let limit = min(ShareLimits.maxFileBytes, ShareLimits.maxItemBytes - total)
            guard let copied = copyBounded(file.url, to: target, limit: limit) else {
                try? FileManager.default.removeItem(at: target)
                skipped += 1
                continue
            }
            total += copied
            let mime = file.mime.isEmpty ? "application/octet-stream" : file.mime
            staged.append(DraftAttachment(path: target.path, name: name, mime: mime, size: copied))
        }
        return StagedAttachments(attachments: staged, skipped: skipped)
    }

    /// The bytes copied, or nil if the source is unreadable or larger than `limit`.
    static func copyBounded(_ source: URL, to target: URL, limit: Int64) -> Int64? {
        guard let input = InputStream(url: source), let output = OutputStream(url: target, append: false) else { return nil }
        input.open()
        output.open()
        defer {
            input.close()
            output.close()
        }
        var buffer = [UInt8](repeating: 0, count: bufferSize)
        var copied: Int64 = 0
        while true {
            let read = input.read(&buffer, maxLength: buffer.count)
            if read < 0 { return nil }
            if read == 0 { break }
            copied += Int64(read)
            if copied > limit { return nil }
            var offset = 0
            while offset < read {
                let written = buffer[offset..<read].withUnsafeBufferPointer { output.write($0.baseAddress!, maxLength: read - offset) }
                if written <= 0 { return nil }
                offset += written
            }
        }
        return copied
    }

    /// The last path component, without control characters or path separators, 120 characters at most.
    static func safeName(_ raw: String, index: Int) -> String {
        let base = raw.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last.map(String.init) ?? ""
        let cleaned = String(base.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) && $0 != ":" })
            .trimmingCharacters(in: .whitespaces)
            .trimmingCharacters(in: CharacterSet(charactersIn: "."))
        return cleaned.isEmpty ? "file-\(index)" : String(cleaned.prefix(maxName))
    }
}

private func directory(root: URL, draftId: String) -> URL {
    root.appendingPathComponent(draftId, isDirectory: true)
}
