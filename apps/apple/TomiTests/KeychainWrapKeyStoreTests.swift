import Foundation
import Security
import TomiRuntime
import XCTest

/// The Keychain store needs a host app (a hostless test bundle has no Keychain access).
final class KeychainWrapKeyStoreTests: XCTestCase {
    private var store: KeychainWrapKeyStore!

    override func setUpWithError() throws {
        store = KeychainWrapKeyStore(service: "eu.shaffe.tomi.tests.\(UUID().uuidString)")
    }

    override func tearDownWithError() throws {
        try store.remove()
    }

    func testTheKeyIsCreatedOnceThenLoaded() throws {
        XCTAssertNil(try store.load())
        let key = try WrapKey.loadOrCreate(store: store, databaseExists: false)
        XCTAssertEqual(key.count, WrapKey.length)
        XCTAssertEqual(try store.load(), key)
        XCTAssertEqual(try WrapKey.loadOrCreate(store: store, databaseExists: true), key)
    }

    func testASecondAddIsReportedAsADuplicate() throws {
        try store.save(Data(repeating: 1, count: 32))
        XCTAssertThrowsError(try store.save(Data(repeating: 2, count: 32))) { error in
            XCTAssertEqual(error as? WrapKeyError, .keychain(errSecDuplicateItem))
        }
        try store.replace(Data(repeating: 3, count: 32))
        XCTAssertEqual(try store.load(), Data(repeating: 3, count: 32))
    }

    func testNoKeyIsInventedForAnExistingDatabase() {
        XCTAssertThrowsError(try WrapKey.loadOrCreate(store: store, databaseExists: true)) { error in
            XCTAssertEqual(error as? WrapKeyError, .missingForExistingDatabase)
        }
    }
}
