import Foundation

/// The shared container of the app and its share extension (App Group): the inbox of shares and the few values the
/// extension reads.
public enum AppGroup {
    public static let identifier = "group.eu.shaffe.tomi"
    /// Posted (Darwin notification) by the extension when a share is put in the inbox.
    public static let inboxChanged = "eu.shaffe.tomi.share-inbox"
    /// `UserDefaults(suiteName: identifier)` key: this device is in a space (written by the app, read by the extension).
    public static let isPairedKey = "core.isPaired"
    /// Key: the app could not open its data ("Données illisibles"); the extension says so instead of taking a share.
    public static let isUnreadableKey = "core.isUnreadable"
    /// Key: how many shares were dropped (older than `ShareInbox.maxAge`) and not yet told to the user; the app says so
    /// once, on screen, and resets it.
    public static let expiredSharesKey = "inbox.expiredShares"

    /// The container, nil when the app group is not in the entitlements.
    public static func container(fileManager: FileManager = .default) -> URL? {
        fileManager.containerURL(forSecurityApplicationGroupIdentifier: identifier)
    }

    public static var defaults: UserDefaults? { UserDefaults(suiteName: identifier) }
}

/// One share put in the inbox by the extension: what the app hands to its core once `notBefore` is past. The files are
/// beside it (`files/0`, `files/1`...), in the order of `files`; their names here are only names, never paths.
public struct ShareHandoff: Codable, Hashable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public struct File: Codable, Hashable, Sendable {
        public var name: String
        public var mime: String
        public var size: Int64
    }

    public static let currentVersion = 1

    public var version: Int
    public var id: String
    public var createdAt: Date
    /// End of the undo window held by the extension: never sent before.
    public var notBefore: Date
    public var title: String?
    public var url: String?
    public var plain: String?
    public var html: String?
    public var markdown: String?
    public var files: [File]
    public var looksSecret: Bool
    public var secret: Bool
    public var expiry: ExpiryChoice

    /// What another app shared is content: never printed.
    public var description: String { "ShareHandoff(\(id), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// The inbox of the share extension, in the app group (`share-inbox/`). The extension puts a share there at once with
/// the end of its undo window, and takes it back on "Annuler"; the app sends what is past that end. Each share is a
/// directory made aside then renamed into place, so the app never sees half of one; the app renames it again before
/// sending (`claim`), so a late "Annuler" finds nothing and says it is too late. Nothing here talks to the core.
public struct ShareInbox: Sendable {
    public static let manifestName = "share.json"
    /// Leftovers of an extension stopped while copying, or of an app stopped while sending, are dropped after this.
    public static let staleAfter: TimeInterval = 24 * 3600
    /// A share that nothing sent in this time is dropped (`dropExpired`): the inbox is plaintext at rest (class A, out of
    /// backups), so it must not keep content for days when the app is not opened.
    public static let maxAge: TimeInterval = 24 * 3600

    public let root: URL

    public init(root: URL) {
        self.root = root
    }

    /// `share-inbox/` in the app group, nil without the app group.
    public static func shared(fileManager: FileManager = .default) -> ShareInbox? {
        AppGroup.container(fileManager: fileManager).map { ShareInbox(root: $0.appendingPathComponent("share-inbox", isDirectory: true)) }
    }

    private var fileManager: FileManager { .default }
    private static let incomingPrefix = ".incoming-"
    private static let buildingPrefix = ".building-"
    private static let claimedPrefix = ".claimed-"
    private static let trashPrefix = ".trash-"

    // MARK: Extension side

    /// A new directory where the extension copies the files it receives before they are parsed.
    public func makeIncomingDirectory() throws -> URL {
        try prepareRoot()
        let dir = root.appendingPathComponent(Self.incomingPrefix + UUID().uuidString.lowercased(), isDirectory: true)
        try fileManager.createDirectory(at: dir, withIntermediateDirectories: true)
        protect(dir)
        return dir
    }

    /// Puts `parsed` in the inbox, to be sent from `now + window`. The files are linked (or copied) beside it: the
    /// originals stay where they are, for "Options" and a second try.
    public func deposit(
        _ parsed: ParsedShare, secret: Bool, expiry: ExpiryChoice, id: String = UUID().uuidString.lowercased(),
        now: Date, window: TimeInterval
    ) throws -> ShareHandoff {
        try prepareRoot()
        let building = root.appendingPathComponent(Self.buildingPrefix + id, isDirectory: true)
        let filesDir = building.appendingPathComponent("files", isDirectory: true)
        try? fileManager.removeItem(at: building)
        try fileManager.createDirectory(at: filesDir, withIntermediateDirectories: true)
        protect(building)
        protect(filesDir)
        do {
            var files: [ShareHandoff.File] = []
            for file in parsed.files {
                let target = filesDir.appendingPathComponent(String(files.count))
                do {
                    try fileManager.linkItem(at: file.url, to: target)
                } catch {
                    try fileManager.copyItem(at: file.url, to: target)
                }
                excludeFromBackup(target)
                files.append(ShareHandoff.File(name: file.name, mime: file.mime, size: file.size))
            }
            let handoff = ShareHandoff(
                version: ShareHandoff.currentVersion, id: id, createdAt: now, notBefore: now.addingTimeInterval(window),
                title: parsed.title, url: parsed.url, plain: parsed.text?.plain, html: parsed.text?.html,
                markdown: parsed.text?.markdown, files: files, looksSecret: parsed.looksSecret, secret: secret, expiry: expiry
            )
            let encoder = JSONEncoder()
            encoder.dateEncodingStrategy = .millisecondsSince1970
            try encoder.encode(handoff).write(to: building.appendingPathComponent(Self.manifestName), options: Self.writeOptions)
            try fileManager.moveItem(at: building, to: directory(id))
            return handoff
        } catch {
            try? fileManager.removeItem(at: building)
            throw error
        }
    }

    /// "Annuler": takes the share back. False when it is no longer there (the app already took it: too late).
    public func withdraw(id: String) -> Bool {
        guard Self.isSafeId(id) else { return false }
        let trash = root.appendingPathComponent(Self.trashPrefix + id, isDirectory: true)
        try? fileManager.removeItem(at: trash)
        do {
            try fileManager.moveItem(at: directory(id), to: trash)
        } catch {
            return false
        }
        try? fileManager.removeItem(at: trash)
        return true
    }

    // MARK: App side

    /// The shares ready to send at `now`, oldest first. A share whose end of window is further than `maxWindow` ahead
    /// is ready too (the clock was set back). Unreadable ones are dropped.
    public func ready(now: Date, maxWindow: TimeInterval) -> [ShareHandoff] {
        pending().filter { $0.notBefore <= now || $0.notBefore.timeIntervalSince(now) > maxWindow }
    }

    /// When the next waiting share becomes ready, nil when none waits.
    public func nextReadyDate(now: Date, maxWindow: TimeInterval) -> Date? {
        pending().map(\.notBefore).filter { $0 > now && $0.timeIntervalSince(now) <= maxWindow }.min()
    }

    /// Takes a share out of the extension's reach before sending it: its directory, or nil when it was withdrawn.
    public func claim(_ handoff: ShareHandoff) -> URL? {
        let claimed = root.appendingPathComponent(Self.claimedPrefix + handoff.id, isDirectory: true)
        do {
            try fileManager.moveItem(at: directory(handoff.id), to: claimed)
            return claimed
        } catch {
            return nil
        }
    }

    /// Gives a claimed share back to the inbox (its sending failed): tried again at the next look. False when it
    /// cannot (it then stays claimed and goes at the next start).
    @discardableResult
    public func unclaim(_ claimed: URL, id: String) -> Bool {
        guard Self.isSafeId(id) else { return false }
        return (try? fileManager.moveItem(at: claimed, to: directory(id))) != nil
    }

    /// The files of a claimed share, as the send screen knows them.
    public func files(of handoff: ShareHandoff, in claimed: URL) -> [SharedFile] {
        handoff.files.enumerated().map { index, file in
            SharedFile(
                url: claimed.appendingPathComponent("files", isDirectory: true).appendingPathComponent(String(index)),
                name: file.name, mime: file.mime, size: file.size
            )
        }
    }

    /// The share as the parser made it.
    public func parsed(_ handoff: ShareHandoff, in claimed: URL) -> ParsedShare {
        ParsedShare(
            title: handoff.title,
            text: handoff.plain.map { RichText(plain: $0, html: handoff.html, markdown: handoff.markdown) },
            url: handoff.url, files: files(of: handoff, in: claimed), looksSecret: handoff.looksSecret
        )
    }

    /// The claimed share is with the core (or could not be): its directory goes.
    public func finish(_ claimed: URL) {
        try? fileManager.removeItem(at: claimed)
    }

    /// At start: drops the leftovers older than `staleAfter` (copies, half-made shares) and every claimed share of an
    /// app stopped while sending (sending it again could send it twice).
    public func purge(now: Date) {
        guard let names = try? fileManager.contentsOfDirectory(atPath: root.path) else { return }
        for name in names where name.hasPrefix(".") {
            let url = root.appendingPathComponent(name)
            let old = ((try? fileManager.attributesOfItem(atPath: url.path))?[.modificationDate] as? Date)
                .map { now.timeIntervalSince($0) > Self.staleAfter } ?? true
            if name.hasPrefix(Self.claimedPrefix) || name.hasPrefix(Self.trashPrefix) || old {
                try? fileManager.removeItem(at: url)
            }
        }
    }

    /// Drops the shares that waited longer than `maxAge` (the app was not opened, or sending kept failing) and returns how
    /// many. The age is the directory's creation date, kept by the renames of `claim` and `unclaim`: no manifest is read,
    /// which the data protection would refuse while the device is locked (a background task). Only unclaimed shares: one
    /// being sent is dot-prefixed and left to `purge`.
    @discardableResult
    public func dropExpired(now: Date, maxAge: TimeInterval = ShareInbox.maxAge) -> Int {
        guard let names = try? fileManager.contentsOfDirectory(atPath: root.path) else { return 0 }
        // An inbox made by a build that did not exclude it from backups: done at the app's next look.
        excludeFromBackup(root)
        var dropped = 0
        for name in names where !name.hasPrefix(".") {
            let url = directory(name)
            guard let attributes = try? fileManager.attributesOfItem(atPath: url.path),
                  let created = (attributes[.creationDate] as? Date) ?? (attributes[.modificationDate] as? Date),
                  now.timeIntervalSince(created) > maxAge
            else { continue }
            if (try? fileManager.removeItem(at: url)) != nil { dropped += 1 }
        }
        return dropped
    }

    /// Everything goes (this device left its space).
    public func removeAll() {
        try? fileManager.removeItem(at: root)
    }

    // MARK: Helpers

    /// Shares are read only with the device unlocked (the extension on screen, the app in the foreground): the
    /// strongest protection class, inherited by the files made inside.
    private static var writeOptions: Data.WritingOptions {
        #if os(iOS)
        [.atomic, .completeFileProtection]
        #else
        [.atomic]
        #endif
    }

    /// The inbox directory, out of backups (iCloud, Finder): a share is plaintext, a secret one included. A backup
    /// exclusion covers what is inside the directory, so it is set on the root, and again on each directory and file made
    ///.
    private func prepareRoot() throws {
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        excludeFromBackup(root)
    }

    private func excludeFromBackup(_ url: URL) {
        var url = url
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? url.setResourceValues(values)
    }

    private func protect(_ url: URL) {
        excludeFromBackup(url)
        #if os(iOS)
        try? fileManager.setAttributes([.protectionKey: FileProtectionType.complete], ofItemAtPath: url.path)
        #endif
    }

    private func directory(_ id: String) -> URL {
        root.appendingPathComponent(id, isDirectory: true)
    }

    /// The shares in the inbox, oldest first.
    private func pending() -> [ShareHandoff] {
        guard let names = try? fileManager.contentsOfDirectory(atPath: root.path) else { return [] }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        var shares: [ShareHandoff] = []
        for name in names where !name.hasPrefix(".") {
            let dir = directory(name)
            guard Self.isSafeId(name),
                  let data = try? Data(contentsOf: dir.appendingPathComponent(Self.manifestName)),
                  let handoff = try? decoder.decode(ShareHandoff.self, from: data),
                  handoff.id == name, handoff.version == ShareHandoff.currentVersion
            else {
                try? fileManager.removeItem(at: dir)
                continue
            }
            shares.append(handoff)
        }
        return shares.sorted { $0.createdAt < $1.createdAt }
    }

    /// Letters, digits and dashes only: an id is a directory name, never a path.
    static func isSafeId(_ id: String) -> Bool {
        !id.isEmpty && id.count <= 64 && id.unicodeScalars.allSatisfy { CharacterSet.alphanumerics.contains($0) && $0.isASCII || $0 == "-" }
    }
}
