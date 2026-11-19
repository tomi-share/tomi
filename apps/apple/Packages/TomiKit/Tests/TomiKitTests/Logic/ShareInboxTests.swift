import Foundation
import XCTest
@testable import TomiKit

/// The inbox between the share extension and the app: deposit, "Annuler", claim before sending, leftovers.
final class ShareInboxTests: XCTestCase {
    private var root: URL!
    private var inbox: ShareInbox!
    private let start = Date(timeIntervalSince1970: 1_800_000_000)

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("inbox-\(UUID().uuidString)", isDirectory: true)
        inbox = ShareInbox(root: root.appendingPathComponent("share-inbox", isDirectory: true))
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func parsed(files: [SharedFile] = []) -> ParsedShare {
        ParsedShare(
            title: "Article", text: RichText(plain: "**gras**", markdown: "**gras**"), url: "https://example.org/a",
            files: files, looksSecret: false
        )
    }

    private func sharedFile(_ contents: String, name: String) throws -> SharedFile {
        let dir = try inbox.makeIncomingDirectory()
        let url = dir.appendingPathComponent("copy-\(name)")
        try Data(contents.utf8).write(to: url)
        return SharedFile(url: url, name: name, mime: "text/plain", size: Int64(contents.utf8.count))
    }

    func testADepositWaitsForTheEndOfTheWindowThenComesBackWhole() throws {
        let file = try sharedFile("bonjour", name: "note.txt")
        let handoff = try inbox.deposit(parsed(files: [file]), secret: true, expiry: .day1, id: "abc-1", now: start, window: 5)
        XCTAssertTrue(inbox.ready(now: start.addingTimeInterval(4.9), maxWindow: 5).isEmpty)
        XCTAssertEqual(inbox.nextReadyDate(now: start, maxWindow: 5), start.addingTimeInterval(5))
        let ready = inbox.ready(now: start.addingTimeInterval(5), maxWindow: 5)
        XCTAssertEqual(ready, [handoff])
        let claimed = try XCTUnwrap(inbox.claim(handoff))
        let back = inbox.parsed(handoff, in: claimed)
        XCTAssertEqual(back.title, "Article")
        XCTAssertEqual(back.url, "https://example.org/a")
        XCTAssertEqual(back.text?.markdown, "**gras**")
        XCTAssertEqual(back.files.map(\.name), ["note.txt"])
        XCTAssertEqual(try String(contentsOf: back.files[0].url, encoding: .utf8), "bonjour")
        XCTAssertEqual(handoff.secret, true)
        XCTAssertEqual(handoff.expiry, .day1)
        XCTAssertTrue(inbox.ready(now: start.addingTimeInterval(60), maxWindow: 5).isEmpty, "claimed: no longer in the inbox")
        inbox.finish(claimed)
        XCTAssertFalse(FileManager.default.fileExists(atPath: claimed.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.url.path), "the original stays for Options")
    }

    func testUndoTakesTheShareBackUnlessTheAppHasIt() throws {
        let first = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "first", now: start, window: 5)
        XCTAssertTrue(inbox.withdraw(id: first.id))
        XCTAssertTrue(inbox.ready(now: start.addingTimeInterval(10), maxWindow: 5).isEmpty)
        let second = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "second", now: start, window: 5)
        XCTAssertNotNil(inbox.claim(second))
        XCTAssertFalse(inbox.withdraw(id: second.id), "too late: the app took it")
        XCTAssertNil(inbox.claim(first), "withdrawn: nothing to send")
    }

    func testAFailedSendGoesBackToTheInbox() throws {
        let share = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "retry", now: start, window: 0)
        let claimed = try XCTUnwrap(inbox.claim(share))
        XCTAssertTrue(inbox.unclaim(claimed, id: share.id))
        XCTAssertEqual(inbox.ready(now: start, maxWindow: 5).map(\.id), ["retry"])
    }

    func testAClockSetBackDoesNotHoldASharePastTheWindow() throws {
        _ = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "late", now: start, window: 5)
        XCTAssertEqual(inbox.ready(now: start.addingTimeInterval(-3600), maxWindow: 5).map(\.id), ["late"])
    }

    func testOldestFirstAndUnreadableOnesDropped() throws {
        _ = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "b", now: start.addingTimeInterval(1), window: 0)
        _ = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "a", now: start, window: 0)
        let broken = inbox.root.appendingPathComponent("broken")
        try FileManager.default.createDirectory(at: broken, withIntermediateDirectories: true)
        try Data("{".utf8).write(to: broken.appendingPathComponent(ShareInbox.manifestName))
        XCTAssertEqual(inbox.ready(now: start.addingTimeInterval(2), maxWindow: 5).map(\.id), ["a", "b"])
        XCTAssertFalse(FileManager.default.fileExists(atPath: broken.path))
    }

    func testTheInboxAndItsSharesAreExcludedFromBackup() throws {
        let file = try sharedFile("secret", name: "a.txt")
        _ = try inbox.deposit(parsed(files: [file]), secret: true, expiry: .never, id: "kept", now: start, window: 0)
        let excluded = { (url: URL) in try url.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup }
        XCTAssertEqual(try excluded(inbox.root), true)
        XCTAssertEqual(try excluded(inbox.root.appendingPathComponent("kept", isDirectory: true)), true)
        XCTAssertEqual(try excluded(inbox.root.appendingPathComponent("kept/files/0")), true)
    }

    func testSharesOlderThanTheMaxAgeAreDroppedWithoutReadingThem() throws {
        _ = try inbox.deposit(parsed(), secret: true, expiry: .never, id: "old", now: start, window: 0)
        XCTAssertEqual(inbox.dropExpired(now: Date()), 0, "fresh")
        XCTAssertEqual(inbox.dropExpired(now: Date().addingTimeInterval(ShareInbox.maxAge - 60)), 0, "not yet")
        let claimedShare = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "claimed", now: start, window: 0)
        let claimed = try XCTUnwrap(inbox.claim(claimedShare))
        XCTAssertEqual(inbox.dropExpired(now: Date().addingTimeInterval(ShareInbox.maxAge + 60)), 1)
        XCTAssertTrue(FileManager.default.fileExists(atPath: claimed.path), "a share being sent is left to purge")
        XCTAssertTrue(inbox.ready(now: start, maxWindow: 5).isEmpty)
    }

    func testPurgeDropsClaimedSharesAndOldLeftoversOnly() throws {
        let share = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "kept", now: start, window: 0)
        let claimedShare = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "claimed", now: start, window: 0)
        let claimed = try XCTUnwrap(inbox.claim(claimedShare))
        let fresh = try inbox.makeIncomingDirectory()
        inbox.purge(now: Date())
        XCTAssertFalse(FileManager.default.fileExists(atPath: claimed.path), "never sent twice")
        XCTAssertTrue(FileManager.default.fileExists(atPath: fresh.path), "an extension may still be copying")
        XCTAssertEqual(inbox.ready(now: start, maxWindow: 5).map(\.id), [share.id])
        inbox.purge(now: Date().addingTimeInterval(ShareInbox.staleAfter + 60))
        XCTAssertFalse(FileManager.default.fileExists(atPath: fresh.path))
        inbox.removeAll()
        XCTAssertTrue(inbox.ready(now: start, maxWindow: 5).isEmpty)
    }

    func testAnIdIsNeverAPath() {
        XCTAssertTrue(ShareInbox.isSafeId("4f1c-aa09"))
        XCTAssertFalse(ShareInbox.isSafeId("../x"))
        XCTAssertFalse(ShareInbox.isSafeId(".claimed-x"))
        XCTAssertFalse(ShareInbox.isSafeId(""))
        XCTAssertFalse(inbox.withdraw(id: "../share-inbox"))
    }

    func testTheHandoffIsNeverPrinted() throws {
        let handoff = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "p", now: start, window: 0)
        XCTAssertFalse(String(describing: handoff).contains("Article"))
    }
}
