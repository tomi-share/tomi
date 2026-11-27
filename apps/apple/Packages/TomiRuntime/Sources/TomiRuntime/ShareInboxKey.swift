import Foundation
import Security
import TomiKit

/// The key the app opens the share inbox with (`ShareSeal`): an X25519 private key in the Keychain of the app alone (no
/// access group: the share extension never gets it, it only receives the public half through the app group). This device
/// only, usable after the first unlock like the wrapping key of the core.
public enum ShareInboxKey {
    public static let service = "eu.shaffe.tomi.share-inbox"
    public static let account = "x25519"

    public static func defaultStore() -> WrapKeyStoring {
        KeychainWrapKeyStore(service: service, account: account)
    }

    /// The stored key, or a new one when none is stored (first start, or a Keychain that was reset: the shares sealed to
    /// the lost key are then unreadable and dropped, which is all it costs).
    public static func loadOrCreate(store: WrapKeyStoring = ShareInboxKey.defaultStore()) throws -> ShareSeal.KeyPair {
        if let stored = try store.load(), let publicKey = try? ShareSeal.publicKey(of: stored) {
            return ShareSeal.KeyPair(privateKey: stored, publicKey: publicKey)
        }
        let pair = ShareSeal.makeKeyPair()
        do {
            try store.save(pair.privateKey)
        } catch WrapKeyError.keychain(errSecDuplicateItem) {
            // Made meanwhile by another start: that one is used, so every part of the app seals to the same key.
            if let stored = try store.load(), let publicKey = try? ShareSeal.publicKey(of: stored) {
                return ShareSeal.KeyPair(privateKey: stored, publicKey: publicKey)
            }
            // A malformed leftover (the stored value did not parse above): replaced.
            try store.replace(pair.privateKey)
        }
        return pair
    }
}
