import AppKit
import UniformTypeIdentifiers

/// What the files of an item do with the system on the Mac (the iOS counterparts are in `FeedViewModel+Files`):
/// the clipboard, the default app, the share menu (and `MacDownloads` for "Enregistrer").
@MainActor
enum MacFileActions {
    /// "Copier l'image": the file on the clipboard under its own type.
    static func copyToClipboard(_ data: Data, type: UTType) {
        let board = NSPasteboard.general
        board.clearContents()
        board.setData(data, forType: NSPasteboard.PasteboardType(type.identifier))
        ClipboardWatcher.shared.noticeOwnCopy()
    }

    /// "Ouvrir": the app the Mac chose for this kind of file. Nothing received from another device is run or
    /// installed from here: a file that is a program, or that the Mac would hand to an app that runs or installs what
    /// it is given, is shown in the Finder instead (opening it there is the user's own, informed, gesture).
    static func open(_ file: URL) -> Bool {
        if runsOrInstalls(file) {
            NSWorkspace.shared.activateFileViewerSelecting([file])
            return true
        }
        return NSWorkspace.shared.open(file)
    }

    /// Apps that run, install or follow what they are given.
    private static let unsafeOpeners: Set<String> = [
        "com.apple.Terminal", "com.apple.ScriptEditor2", "com.apple.installer", "com.apple.JarLauncher",
        "com.apple.archiveutility", "com.apple.Automator", "com.apple.DiskImageMounter", "com.apple.systempreferences",
        "com.apple.ProfileHelper", "com.apple.ShortcutsActions", "com.apple.shortcuts",
    ]

    /// Extensions of files that run, install or point somewhere, whatever app would take them.
    private static let unsafeExtensions: Set<String> = [
        "app", "command", "tool", "terminal", "sh", "zsh", "bash", "csh", "fish", "py", "rb", "pl", "php", "js",
        "scpt", "scptd", "applescript", "workflow", "action", "shortcut", "jar", "pkg", "mpkg", "dmg",
        "mobileconfig", "webloc", "inetloc", "fileloc", "url", "prefpane", "qlgenerator", "saver", "osax",
    ]

    static func runsOrInstalls(_ file: URL) -> Bool {
        if unsafeExtensions.contains(file.pathExtension.lowercased()) { return true }
        let type = (try? file.resourceValues(forKeys: [.contentTypeKey]))?.contentType
            ?? UTType(filenameExtension: file.pathExtension)
        let runnable: [UTType] = [.executable, .application, .script, .shellScript, .unixExecutable, .bundle, .archive, .diskImage]
        if let type, runnable.contains(where: { type.conforms(to: $0) }) { return true }
        if let opener = NSWorkspace.shared.urlForApplication(toOpen: file),
           let identifier = Bundle(url: opener)?.bundleIdentifier, unsafeOpeners.contains(identifier) {
            return true
        }
        return false
    }

    /// The share menu of the system for `copy` (a file in a folder of its own, erased once the share is over). A
    /// hidden item is only offered to services that send it to someone: no save, no copy, no print on this Mac (what
    /// Mail or Messages keep of what they sent is theirs).
    static func share(_ copy: URL, sensitive: Bool) -> Bool {
        share(items: [copy], subject: nil, sensitive: sensitive, folder: copy.deletingLastPathComponent())
    }

    /// The share menu for `items` (a text, a file). `folder`: erased once the share is over (the copy of a file).
    static func share(items: [Any], subject: String?, sensitive: Bool, folder: URL?) -> Bool {
        guard let view = NSApp.keyWindow?.contentView else {
            if let folder { try? FileManager.default.removeItem(at: folder) }
            return false
        }
        let session = ShareSession(folder: folder, subject: subject, sensitive: sensitive)
        current?.finish()
        current = session
        let picker = NSSharingServicePicker(items: items)
        picker.delegate = session
        let anchor = NSRect(x: view.bounds.midX, y: view.bounds.midY, width: 1, height: 1)
        picker.show(relativeTo: anchor, of: view, preferredEdge: .minY)
        return true
    }

    /// The share in progress (the picker holds its delegate weakly).
    private static var current: ShareSession?

    fileprivate static func ended(_ session: ShareSession) {
        if current === session { current = nil }
    }
}

/// One use of the share menu: filters the services of a hidden item and erases the shared copy at the end.
@MainActor
private final class ShareSession: NSObject, NSSharingServicePickerDelegate, NSSharingServiceDelegate {
    private let folder: URL?
    private let subject: String?
    private let sensitive: Bool

    /// What a hidden item may go through: sent to someone; no service that saves, copies or prints it here.
    private static let allowedForSecret: Set<NSSharingService.Name> = [.sendViaAirDrop, .composeMessage, .composeEmail]

    init(folder: URL?, subject: String?, sensitive: Bool) {
        self.folder = folder
        self.subject = subject
        self.sensitive = sensitive
    }

    func finish() {
        if let folder { try? FileManager.default.removeItem(at: folder) }
        MacFileActions.ended(self)
    }

    func sharingServicePicker(
        _ picker: NSSharingServicePicker, sharingServicesForItems items: [Any], proposedSharingServices proposed: [NSSharingService]
    ) -> [NSSharingService] {
        guard sensitive else { return proposed }
        return Self.allowedForSecret.compactMap { NSSharingService(named: $0) }.filter { $0.canPerform(withItems: items) }
    }

    func sharingServicePicker(_ picker: NSSharingServicePicker, delegateFor service: NSSharingService) -> NSSharingServiceDelegate? {
        self
    }

    func sharingServicePicker(_ picker: NSSharingServicePicker, didChoose service: NSSharingService?) {
        // Closed without a choice: nothing will read the copy.
        guard let service else { return finish() }
        if let subject { service.subject = subject }
    }

    func sharingService(_ service: NSSharingService, didShareItems items: [Any]) {
        finish()
    }

    func sharingService(_ service: NSSharingService, didFailToShareItems items: [Any], error: Error) {
        finish()
    }
}

/// Where "Save" writes: the folder chosen in the settings, else `Downloads/Tomi`. No dialog at each
/// save (the Windows app does the same); never used for a hidden item.
@MainActor
enum MacDownloads {
    private static let bookmarkKey = "downloadFolderBookmark"

    /// The folder chosen in the settings (a security-scoped bookmark: the sandbox only lets the app back in through
    /// it), nil when none was chosen or when it is gone.
    private static func chosenFolder(defaults: UserDefaults = .standard) -> URL? {
        guard let bookmark = defaults.data(forKey: bookmarkKey) else { return nil }
        var stale = false
        guard let url = try? URL(resolvingBookmarkData: bookmark, options: .withSecurityScope, bookmarkDataIsStale: &stale) else {
            return nil
        }
        // A folder put in the Trash still resolves: it is no longer a place to save in.
        if url.pathComponents.contains(".Trash") { return nil }
        if stale, url.startAccessingSecurityScopedResource() {
            // Moved or renamed since: a fresh bookmark of the same folder.
            if let fresh = try? url.bookmarkData(options: .withSecurityScope) { defaults.set(fresh, forKey: bookmarkKey) }
            url.stopAccessingSecurityScopedResource()
        }
        return url
    }

    private static var defaultFolder: URL? {
        FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first?.appendingPathComponent("Tomi", isDirectory: true)
    }

    static var hasChosenFolder: Bool { chosenFolder() != nil }

    /// What the settings show: the name of the chosen folder, or "Downloads › Tomi".
    static var displayName: String {
        chosenFolder().map { FileManager.default.displayName(atPath: $0.path) } ?? L10n.text("macos_downloads_default_name")
    }

    /// Remembers the folder the user picked in the open panel. False when the sandbox gives no lasting access to it.
    static func choose(_ folder: URL, defaults: UserDefaults = .standard) -> Bool {
        guard let bookmark = try? folder.bookmarkData(options: .withSecurityScope) else { return false }
        defaults.set(bookmark, forKey: bookmarkKey)
        return true
    }

    static func reset(defaults: UserDefaults = .standard) {
        defaults.removeObject(forKey: bookmarkKey)
    }

    /// Copies `source` under `name` in the folder (a name already taken gets a number). The saved file, or nil.
    static func save(_ source: URL, name: String) -> URL? {
        let chosen = chosenFolder()
        guard let folder = chosen ?? defaultFolder else { return nil }
        let scoped = chosen?.startAccessingSecurityScopedResource() ?? false
        defer { if scoped { chosen?.stopAccessingSecurityScopedResource() } }
        return copy(source, name: name, into: folder)
    }

    /// The copy itself, in any folder the app may write in.
    static func copy(_ source: URL, name: String, into folder: URL) -> URL? {
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let target = freeName(name, in: folder)
            try FileManager.default.copyItem(at: source, to: target)
            // The copy is the user's: writable, whatever the source was (the cache of the core is read-only).
            try? FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: target.path)
            return target
        } catch {
            return nil
        }
    }

    /// `name` in `folder`, or "name (2).ext", "name (3).ext"… when it is taken: a save never replaces a file.
    static func freeName(_ name: String, in folder: URL) -> URL {
        let base = (name as NSString).deletingPathExtension
        let ext = (name as NSString).pathExtension
        var candidate = folder.appendingPathComponent(name)
        var number = 2
        while FileManager.default.fileExists(atPath: candidate.path) {
            let numbered = ext.isEmpty ? "\(base) (\(number))" : "\(base) (\(number)).\(ext)"
            candidate = folder.appendingPathComponent(numbered)
            number += 1
        }
        return candidate
    }
}
