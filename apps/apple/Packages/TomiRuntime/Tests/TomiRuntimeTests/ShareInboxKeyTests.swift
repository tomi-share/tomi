import Foundation
import Security
import TomiKit
import XCTest
@testable import TomiRuntime

/// In-memory store (the Keychain is not reachable from a hostless test bundle).
private final class MemoryStore: WrapKeyStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var value: Data?

    init(_ value: Data? = nil) {
        self.value = value
    }

    func load() throws -> Data? {
        lock.withLock { value }
    }

    func save(_ key: Data) throws {
        try lock.withLock {
            if value != nil { throw WrapKeyError.keychain(errSecDuplicateItem) }
            value = key
        }
    }

    func replace(_ key: Data) throws {
        lock.withLock { value = key }
    }
}

final class ShareInboxKeyTests: XCTestCase {
    func testTheFirstStartMakesAKeyAndLaterStartsFindIt() throws {
        let store = MemoryStore()
        let first = try ShareInboxKey.loadOrCreate(store: store)
        XCTAssertEqual(first.publicKey.count, ShareSeal.publicKeyLength)
        let again = try ShareInboxKey.loadOrCreate(store: store)
        XCTAssertEqual(again.privateKey, first.privateKey, "never a new key while one is stored")
        XCTAssertEqual(again.publicKey, first.publicKey)
        XCTAssertEqual(try ShareSeal.publicKey(of: first.privateKey), first.publicKey)
    }

    func testAMalformedLeftoverIsReplaced() throws {
        let store = MemoryStore(Data([1, 2, 3]))
        let pair = try ShareInboxKey.loadOrCreate(store: store)
        XCTAssertEqual(try store.load(), pair.privateKey)
        XCTAssertEqual(try ShareSeal.publicKey(of: pair.privateKey), pair.publicKey)
    }
}
