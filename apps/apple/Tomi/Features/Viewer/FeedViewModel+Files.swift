import Foundation
import ImageIO
import Photos
import TomiKit
import UIKit
import UniformTypeIdentifiers

/// The image viewer to show over the stack (a full-screen cover).
struct ViewerRequest: Identifiable, Hashable {
    let itemId: String
    let attachmentIndex: Int
    var id: String { "\(itemId)#\(attachmentIndex)" }
}

/// Files of the image viewer and of the code screen (Android `FeedViewModel.imageFile` / `codeFile`, `ImageActions`):
/// a hidden item goes through the same biometric gate and 2-minute window as "Afficher", and nothing is decrypted before.
extension FeedViewModel {
    private var imageActions: ImageActions {
        ImageActions { [weak self] itemId, slot in await self?.attachmentPath(itemId, slot) }
    }

    private func attachmentPath(_ itemId: String, _ slot: Int) async -> String? {
        (try? await core.attachmentPath(itemId: itemId, slot: slot)) ?? nil
    }

    // MARK: Opening

    /// A tap on an image: the viewer (it asks the gate itself for a hidden item).
    func openViewer(_ item: TomiItem, attachmentIndex: Int) {
        onCardOpened(item)
        fileRequest = nil
        viewerRequest = ViewerRequest(itemId: item.id, attachmentIndex: attachmentIndex)
    }

    /// A tap on a code file: the code screen.
    func openCode(_ item: TomiItem, attachmentIndex: Int) {
        onCardOpened(item)
        codeRequest = ViewerRequest(itemId: item.id, attachmentIndex: attachmentIndex)
    }

    /// A tap on attachment `index` of a card or of the detail: each row by its own type.
    func openAttachmentRow(_ item: TomiItem, index: Int) -> Bool {
        guard item.attachments.indices.contains(index) else { return false }
        switch AttachmentRowAction.of(item.attachments[index], imageOpenable: true, codeOpenable: true) {
        case .openImage: openViewer(item, attachmentIndex: index)
        case .openCodeFile: openCode(item, attachmentIndex: index)
        case .none:
            // Another file: the file viewer (a hidden one is not drawn there, only offered to share).
            openAttachment(item, index: index)
        }
        return true
    }

    // MARK: Gate

    /// Authentication to open the viewer (`file` false) or the code screen on a hidden item; true at once for a normal
    /// one. A missing screen lock is said once; a cancelled prompt is silent.
    func authorizeView(_ item: TomiItem, file: Bool) async -> Bool {
        let reason = L10n.text(file ? "biometric_title_file" : "biometric_title_image")
        let result = await imageActions.authorizeView(item) { await gate.authenticate(reason: reason) }
        report(result)
        return result == .done
    }

    /// The decrypted file of a page of the viewer or of a code file; for a hidden item only while the window is open.
    func viewFile(itemId: String, attachmentIndex: Int) async -> URL? {
        let window = gate.window
        let path = await imageActions.fileForView(item(id: itemId), attachmentIndex: attachmentIndex) { window.isValid }
        return path.map { URL(fileURLWithPath: $0) }
    }

    // MARK: Actions

    /// "Enregistrer dans la galerie" (never for a hidden item: no lasting clear copy).
    func saveImage(_ item: TomiItem, attachmentIndex: Int) {
        guard ViewerActions.canSaveToGallery(item) else { return }
        Task {
            let result = await imageActions.save(item, attachmentIndex: attachmentIndex, authenticate: { .success }) { path, attachment in
                await Self.addToPhotos(URL(fileURLWithPath: path), name: GalleryName.of(attachment.name, mime: attachment.mime))
            }
            if result == .done { feedback.show(L10n.text("snackbar_image_saved")) } else { report(result) }
        }
    }

    /// "Partager": a copy under its own name goes to the share sheet and is erased when the sheet closes. A hidden
    /// image asks the gate, and the sheet offers neither "Copier" nor "Enregistrer l'image".
    func shareImage(_ item: TomiItem, attachmentIndex: Int) {
        guard mayShareNow(item) else { return }
        Task {
            let reason = L10n.text("biometric_title_share")
            let result = await imageActions.share(item, attachmentIndex: attachmentIndex, authenticate: { await gate.authenticate(reason: reason) }) { path, attachment in
                Self.presentShare(URL(fileURLWithPath: path), name: GalleryName.of(attachment.name, mime: attachment.mime), sensitive: item.secret)
            }
            report(result)
        }
    }

    /// "Partager" any file of the item: a hidden item asks the gate first; the copy handed to the sheet carries the
    /// name of the file and goes when the sheet closes.
    func shareAttachment(_ item: TomiItem, index: Int) {
        guard item.isReadable, item.attachments.indices.contains(index), mayShareNow(item) else { return }
        let attachment = item.attachments[index]
        Task {
            // Checked again after the prompt: the screen may have started being shared meanwhile.
            guard await unlockForAction(item, reason: "biometric_title_share"), mayShareNow(item) else { return }
            guard let path = await attachmentPath(item.id, item.attachments.slot(at: index)) else {
                feedback.show(L10n.text("snackbar_image_unavailable"))
                return
            }
            if !Self.presentShare(URL(fileURLWithPath: path), name: Self.fileName(attachment), sensitive: item.secret) {
                feedback.show(L10n.text("snackbar_image_failed"))
            }
        }
    }

    /// "Copier l'image": the image file on the clipboard, under its own type (paste in Messages, Notes...). Never for a
    /// hidden item, like "Enregistrer dans la galerie": the clipboard is read by other apps.
    func copyImage(_ item: TomiItem, attachmentIndex: Int) {
        guard ViewerActions.canSaveToGallery(item), item.attachments.indices.contains(attachmentIndex) else { return }
        let attachment = item.attachments[attachmentIndex]
        Task {
            guard let path = await attachmentPath(item.id, item.attachments.slot(at: attachmentIndex)) else {
                feedback.show(L10n.text("snackbar_image_unavailable"))
                return
            }
            let data = await Task.detached(priority: .userInitiated) { try? Data(contentsOf: URL(fileURLWithPath: path)) }.value
            guard let data else {
                feedback.show(L10n.text("snackbar_image_failed"))
                return
            }
            let type = UTType(mimeType: attachment.mime) ?? UTType(filenameExtension: (attachment.name as NSString).pathExtension) ?? .image
            UIPasteboard.general.setData(data, forPasteboardType: type.identifier)
            onCardOpened(item)
            feedback.show(L10n.text("snackbar_copied"))
        }
    }

    /// "Enregistrer dans Fichiers": the system picker of a place. Never for a hidden item (no lasting clear copy, as
    /// "Enregistrer dans la galerie").
    func saveAttachmentToFiles(_ item: TomiItem, index: Int) {
        guard !item.secret, item.isReadable, item.attachments.indices.contains(index) else { return }
        let attachment = item.attachments[index]
        Task {
            guard let path = await attachmentPath(item.id, item.attachments.slot(at: index)) else {
                feedback.show(L10n.text("snackbar_image_unavailable"))
                return
            }
            if !Self.presentExport(URL(fileURLWithPath: path), name: Self.fileName(attachment)) {
                feedback.show(L10n.text("snackbar_image_failed"))
            }
        }
    }

    /// A hidden item is not shared while the screen is shared or recorded: the share sheet shows the file (its name, a
    /// preview) above every cover of the app.
    private func mayShareNow(_ item: TomiItem) -> Bool {
        guard item.secret, Self.screenCaptured else { return true }
        feedback.show(L10n.text("ios_capture_share_blocked"))
        return false
    }

    private static var screenCaptured: Bool {
        UIApplication.shared.connectedScenes.contains { ($0 as? UIWindowScene)?.screen.isCaptured == true }
    }

    /// True when the action may go on: at once for a normal item, after the gate for a hidden one.
    private func unlockForAction(_ item: TomiItem, reason: String) async -> Bool {
        guard item.secret else { return true }
        switch await gate.authenticate(reason: L10n.text(reason)) {
        case .success: return true
        case .unavailable:
            feedback.show(L10n.text("toast_biometric_unavailable"))
            return false
        case .cancelled, .failed: return false
        }
    }

    /// The decrypted file under its own name, for Quick Look and the player (they read the type from the name; the cache
    /// of the core names files by item). A hard link in a folder of its own: no second clear copy, gone with `removeViewLink`
    /// or at the next start.
    nonisolated static func viewLink(_ source: URL, name: String) -> URL? {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("view-\(UUID().uuidString)", isDirectory: true)
        let link = folder.appendingPathComponent(name)
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try FileManager.default.linkItem(at: source, to: link)
            return link
        } catch {
            try? FileManager.default.removeItem(at: folder)
            return nil
        }
    }

    nonisolated static func removeViewLink(_ link: URL) {
        try? FileManager.default.removeItem(at: link.deletingLastPathComponent())
    }

    /// The name of a file as another app receives it: the last component, without path or control characters.
    static func fileName(_ attachment: Attachment) -> String {
        if attachment.isImage { return GalleryName.of(attachment.name, mime: attachment.mime) }
        let last = attachment.name.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last.map(String.init) ?? ""
        var clean = String(last.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) && $0 != ":" })
            .trimmingCharacters(in: .whitespaces)
        // Neither "." nor ".." nor a hidden name: the copy must land in its own folder, visible.
        while clean.hasPrefix(".") { clean.removeFirst() }
        return clean.isEmpty ? "fichier" : String(clean.prefix(120))
    }

    /// "Copier le fichier" of the code screen (a hidden item through the sensitive clipboard; the gate already passed).
    func copyCode(_ item: TomiItem, text: String) {
        SensitiveClipboard.copy(plain: text, sensitive: item.secret)
        Task {
            if !item.read || item.readPolicy == .afterFirstRead { try? await core.markRead(id: item.id) }
        }
        feedback.show(L10n.text(item.secret ? "snackbar_copied_secret" : "snackbar_copied"))
    }

    private func report(_ result: ImageActionResult) {
        switch result {
        case .done, .notAuthenticated: break
        case .unavailable: feedback.show(L10n.text("snackbar_image_unavailable"))
        case .failed: feedback.show(L10n.text("snackbar_image_failed"))
        case .biometricUnavailable: feedback.show(L10n.text("toast_biometric_unavailable"))
        }
    }

    // MARK: System

    /// Adds the file to the photo library (permission "add only", asked the first time).
    private static func addToPhotos(_ url: URL, name: String) async -> Bool {
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else { return false }
        do {
            try await PHPhotoLibrary.shared().performChanges {
                let options = PHAssetResourceCreationOptions()
                options.originalFilename = name
                PHAssetCreationRequest.forAsset().addResource(with: .photo, fileURL: url, options: options)
            }
            return true
        } catch {
            return false
        }
    }

    /// The system picker that saves a copy where the user chooses (Files, iCloud Drive...).
    private static func presentExport(_ source: URL, name: String) -> Bool {
        guard let presenter = SystemShare.topViewController(), let copy = temporaryCopy(source, name: name) else { return false }
        let picker = UIDocumentPickerViewController(forExporting: [copy], asCopy: true)
        // The copy goes as soon as the picker closes, saved or not.
        let cleanup = ExportCleanup(folder: copy.deletingLastPathComponent())
        exportCleanup = cleanup
        picker.delegate = cleanup
        presenter.present(picker, animated: true)
        return true
    }

    /// The delegate of the open export picker (the picker holds it weakly).
    private static var exportCleanup: ExportCleanup?

    /// The sheet sharing a hidden item, closed if the screen starts being shared or recorded meanwhile.
    private static weak var sensitiveSheet: UIViewController?

    /// The screen is now shared or recorded: a share sheet of a hidden item goes (its copy with it).
    static func screenCaptureStarted() {
        guard let sheet = sensitiveSheet else { return }
        sheet.dismiss(animated: false)
        // A sheet closed from here does not call its completion: its copy goes now.
        if let folder = sensitiveSheetFolder { try? FileManager.default.removeItem(at: folder) }
        sensitiveSheetFolder = nil
    }

    private static var sensitiveSheetFolder: URL?

    /// A copy under its own name in a folder of its own of the temporary directory (swept at start and in the
    /// background), readable only while the device is unlocked.
    private static func temporaryCopy(_ source: URL, name: String) -> URL? {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("\(sharePrefix)\(UUID().uuidString)", isDirectory: true)
        let copy = dir.appendingPathComponent(name)
        do {
            try FileManager.default.createDirectory(
                at: dir, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.complete]
            )
            try FileManager.default.copyItem(at: source, to: copy)
            return copy
        } catch {
            try? FileManager.default.removeItem(at: dir)
            return nil
        }
    }

    private static func presentShare(_ source: URL, name: String, sensitive: Bool) -> Bool {
        // The screen may have started being shared during the Face ID prompt: a hidden item's sheet does not open.
        if sensitive && screenCaptured { return true }
        guard let presenter = SystemShare.topViewController() else { return false }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("\(sharePrefix)\(UUID().uuidString)", isDirectory: true)
        let copy = dir.appendingPathComponent(name)
        do {
            // Readable only while the device is unlocked, like the decrypted cache.
            try FileManager.default.createDirectory(
                at: dir, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.complete]
            )
            try FileManager.default.copyItem(at: source, to: copy)
        } catch {
            try? FileManager.default.removeItem(at: dir)
            return false
        }
        let controller = UIActivityViewController(activityItems: [copy], applicationActivities: nil)
        // A hidden item leaves no lasting clear copy: neither the clipboard, nor Photos, nor Files, nor a shared album.
        if sensitive {
            controller.excludedActivityTypes = [
                .copyToPasteboard, .saveToCameraRoll, .assignToContact, .print, .markupAsPDF,
                UIActivity.ActivityType("com.apple.DocumentManagerUICore.SaveToFiles"),
                UIActivity.ActivityType("com.apple.mobileslideshow.StreamShareService"),
            ]
            sensitiveSheet = controller
            sensitiveSheetFolder = dir
        }
        controller.completionWithItemsHandler = { _, _, _, _ in try? FileManager.default.removeItem(at: dir) }
        controller.popoverPresentationController?.sourceView = presenter.view
        controller.popoverPresentationController?.sourceRect = CGRect(x: presenter.view.bounds.maxX - 40, y: 60, width: 1, height: 1)
        presenter.present(controller, animated: true)
        return true
    }

    private static let sharePrefix = "share-"

    /// The copies handed to a share sheet that never closed (the app was stopped meanwhile): at start and in the background.
    nonisolated static func sweepShareCopies() {
        let tmp = FileManager.default.temporaryDirectory
        let names = (try? FileManager.default.contentsOfDirectory(atPath: tmp.path)) ?? []
        for name in names where name.hasPrefix("share-") || name.hasPrefix("view-") {
            try? FileManager.default.removeItem(at: tmp.appendingPathComponent(name))
        }
    }

    /// Decodes an image for the screen (Android `ImageBitmapLoader`): EXIF orientation applied, at most 4096 px on the
    /// longest side and 12 MP, so a 50 MP photo neither exhausts memory nor exceeds the texture size. Off the main thread.
    nonisolated static func loadImage(_ url: URL) -> UIImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary) else { return nil }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        let width = properties?[kCGImagePropertyPixelWidth] as? Int ?? 0
        let height = properties?[kCGImagePropertyPixelHeight] as? Int ?? 0
        let sample = ImageSampling.sampleSize(width: width, height: height, maxEdge: 4096, maxPixels: 12_000_000)
        let edge = width > 0 && height > 0 ? max(width, height) / sample : 4096
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: max(edge, 1),
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return UIImage(cgImage: image)
    }

    /// Reads a code file into memory (2 MB at most); the decrypted copy of the core cache is read, never copied.
    nonisolated static func readCode(_ url: URL) -> CodeLoadResult {
        guard let input = InputStream(url: url) else { return .unavailable }
        return CodeDecoding.read(input)
    }
}

/// Erases the copy handed to the "Enregistrer dans Fichiers" picker once it closes, saved or cancelled.
@MainActor
private final class ExportCleanup: NSObject, UIDocumentPickerDelegate {
    let folder: URL

    init(folder: URL) { self.folder = folder }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { erase() }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { erase() }

    private func erase() { try? FileManager.default.removeItem(at: folder) }
}
