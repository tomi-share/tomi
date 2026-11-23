import Foundation
import Security

/// Where the core keeps its files: `Application Support/eu.shaffe.tomi/` (the App Group
/// container once the share extension exists), excluded from backups, readable after the first
/// unlock (a sync may run in the background).
public struct CorePaths: Sendable {
    public let root: URL

    public init(root: URL) {
        self.root = root
    }

    public var database: URL { root.appendingPathComponent("tomi.db") }
    public var files: URL { root.appendingPathComponent("files", isDirectory: true) }
    /// Files of the demo core (erased by the core at every opening).
    public var demo: URL { root.appendingPathComponent("demo", isDirectory: true) }
    /// Decrypted attachments, made by the core on demand (`files/files/cache`, see
    /// `crates/core/src/runtime/store.rs`).
    public var plaintextCache: URL { files.appendingPathComponent("files/cache", isDirectory: true) }
    /// Copies of the files of a draft being sent, read by the core during `enqueue` then erased
    /// (`files/outgoing/<draftId>`, Android `cache/outgoing`).
    public var outgoing: URL { files.appendingPathComponent("outgoing", isDirectory: true) }
    /// Files chosen on the send screen and not sent yet (`files/picked/<id>`): a picked file must be
    /// copied at once, the system only lends it for the moment of the choice.
    public var picked: URL { files.appendingPathComponent("picked", isDirectory: true) }

    /// Erases what a previous run left in the send directories (a draft copy is never needed after a
    /// restart: the core keeps its own copy of queued items).
    public func prepareSendDirectories(fileManager: FileManager = .default) throws {
        try? fileManager.removeItem(at: outgoing)
        try fileManager.createDirectory(at: outgoing, withIntermediateDirectories: true)
    }

    /// The app's private directory, created if needed.
    public static func applicationSupport(fileManager: FileManager = .default) throws -> CorePaths {
        let base = try fileManager.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let paths = CorePaths(root: base.appendingPathComponent("eu.shaffe.tomi", isDirectory: true))
        try paths.prepare(fileManager: fileManager)
        return paths
    }

    /// Creates the directory, keeps it out of backups (iCloud, computer) and sets its protection.
    public func prepare(fileManager: FileManager = .default) throws {
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: plaintextCache, withIntermediateDirectories: true)
        #if os(iOS)
        // The database must stay usable by a background sync after the first unlock; the
        // decrypted attachments are only ever made with the app on screen, so they get the
        // strongest class (unreadable while the device is locked). The core empties the cache
        // but keeps the directory, so new files inherit its class.
        try fileManager.setAttributes(
            [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication],
            ofItemAtPath: root.path
        )
        try fileManager.setAttributes([.protectionKey: FileProtectionType.complete], ofItemAtPath: plaintextCache.path)
        #endif
        var url = root
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try url.setResourceValues(values)
    }
}

/// Storage of the 32-byte wrapping key that seals the secrets of the core's database.
public protocol WrapKeyStoring: Sendable {
    func load() throws -> Data?
    /// Adds the key; throws `WrapKeyError.keychain(errSecDuplicateItem)` when one exists.
    func save(_ key: Data) throws
    /// Overwrites an existing key (only when no database depends on it).
    func replace(_ key: Data) throws
}

public enum WrapKeyError: Error, Equatable {
    /// The database exists but its key is gone (restored backup, Keychain reset): a new key would
    /// make the database unreadable for good, so none is created.
    case missingForExistingDatabase
    case keychain(OSStatus)
    case randomGenerator(OSStatus)
}

public enum WrapKey {
    public static let length = 32

    /// The key of the database: loaded, or created when the database does not exist yet. Never a
    /// new key for an existing database (same rule as the Windows app, `wrapkey.rs`).
    public static func loadOrCreate(store: WrapKeyStoring, databaseExists: Bool) throws -> Data {
        if let key = try store.load(), key.count == length {
            return key
        }
        if databaseExists {
            throw WrapKeyError.missingForExistingDatabase
        }
        var bytes = [UInt8](repeating: 0, count: length)
        let status = SecRandomCopyBytes(kSecRandomDefault, length, &bytes)
        guard status == errSecSuccess else { throw WrapKeyError.randomGenerator(status) }
        let key = Data(bytes)
        do {
            try store.save(key)
        } catch WrapKeyError.keychain(errSecDuplicateItem) {
            // Created meanwhile by another start (or the share extension): use that one.
            if let stored = try store.load(), stored.count == length { return stored }
            // A malformed leftover with no database behind it: replace it.
            try store.replace(key)
        }
        return key
    }
}

/// The wrapping key in the Keychain: this device only (`ThisDeviceOnly`, never in iCloud
/// Keychain nor in a backup restored elsewhere), usable after the first unlock so a background
/// sync can open the database.
public struct KeychainWrapKeyStore: WrapKeyStoring {
    public let service: String
    public let account: String
    /// Keychain access group shared with the share extension (nil: the app's default group).
    public let accessGroup: String?

    public init(service: String = "eu.shaffe.tomi.core", account: String = "wrap-key", accessGroup: String? = nil) {
        self.service = service
        self.account = account
        self.accessGroup = accessGroup
    }

    private var baseQuery: [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: false,
        ]
        #if os(macOS)
        // On the Mac the default is the file-based login keychain, which ignores
        // `kSecAttrAccessible` and the access group: ask for the data protection keychain, the
        // one iOS always uses. It needs an app signed with an application identifier.
        query[kSecUseDataProtectionKeychain as String] = true
        #endif
        if let accessGroup { query[kSecAttrAccessGroup as String] = accessGroup }
        return query
    }

    public func load() throws -> Data? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess: return result as? Data
        case errSecItemNotFound: return nil
        default: throw WrapKeyError.keychain(status)
        }
    }

    public func save(_ key: Data) throws {
        var query = baseQuery
        query[kSecValueData as String] = key
        query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else { throw WrapKeyError.keychain(status) }
    }

    public func replace(_ key: Data) throws {
        let attributes: [String: Any] = [
            kSecValueData as String: key,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let status = SecItemUpdate(baseQuery as CFDictionary, attributes as CFDictionary)
        guard status == errSecSuccess else { throw WrapKeyError.keychain(status) }
    }

    /// Deletes the key (tests; a future "reset this device" flow).
    public func remove() throws {
        let status = SecItemDelete(baseQuery as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw WrapKeyError.keychain(status) }
    }
}
