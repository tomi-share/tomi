import Foundation
import XCTest
@testable import TomiKit

/// The inbox between the share extension and the app: deposit, "Annuler", claim before sending, leftovers.
final class ShareInboxTests: XCTestCase {
    private var root: URL!
    private var inbox: ShareInbox!
    private var keys: ShareSeal.KeyPair!
    private let start = Date(timeIntervalSince1970: 1_800_000_000)

    /// The extension's view of the inbox (the public key only) and the app's (the private key only) are the same
    /// directory: `inbox` has both, to deposit and to read in one test.
    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("inbox-\(UUID().uuidString)", isDirectory: true)
        keys = ShareSeal.makeKeyPair()
        inbox = ShareInbox(
            root: root.appendingPathComponent("share-inbox", isDirectory: true), recipientKey: keys.publicKey, privateKey: keys.privateKey
        )
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
        let back = try inbox.parsed(handoff, in: claimed)
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

    // MARK: Sealed at rest

    private func everyByte(under url: URL) throws -> [Data] {
        let walker = try XCTUnwrap(FileManager.default.enumerator(at: url, includingPropertiesForKeys: [.isRegularFileKey]))
        var all: [Data] = []
        for case let file as URL in walker where (try? file.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true {
            all.append(try Data(contentsOf: file))
        }
        return all
    }

    func testNothingInTheInboxIsPlaintext() throws {
        let file = try sharedFile("la phrase du fichier", name: "note.txt")
        let share = ParsedShare(
            title: "Titre à cacher", text: RichText(plain: "le mot de passe est 1234", markdown: "le mot de passe est 1234"),
            url: "https://example.org/secret-path", files: [file], looksSecret: true
        )
        _ = try inbox.deposit(share, secret: true, expiry: .never, id: "sealed", now: start, window: 0)
        // What the extension copies to read the share goes when it closes (`ShareViewController`): only what stays counts.
        try FileManager.default.removeItem(at: file.url.deletingLastPathComponent())
        let contents = try everyByte(under: inbox.root)
        XCTAssertGreaterThanOrEqual(contents.count, 2, "the manifest and the file")
        for phrase in ["la phrase du fichier", "le mot de passe", "Titre à cacher", "secret-path", "note.txt"] {
            for data in contents {
                XCTAssertNil(data.range(of: Data(phrase.utf8)), "\"\(phrase)\" found in the inbox")
            }
        }
    }

    func testTheExtensionHasNoWayToReadWhatItDeposited() throws {
        _ = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "kept", now: start, window: 0)
        let extensionOnly = ShareInbox(root: inbox.root, recipientKey: keys.publicKey)
        XCTAssertTrue(extensionOnly.ready(now: start, maxWindow: 5).isEmpty)
        XCTAssertTrue(FileManager.default.fileExists(atPath: inbox.root.appendingPathComponent("kept").path),
                      "an app that cannot read its key yet must not lose the share")
        XCTAssertEqual(inbox.ready(now: start, maxWindow: 5).map(\.id), ["kept"], "the app opens it")
    }

    func testAShareSealedToAnotherKeyIsDropped() throws {
        _ = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "lost", now: start, window: 0)
        let other = ShareInbox(root: inbox.root, privateKey: ShareSeal.makeKeyPair().privateKey)
        XCTAssertTrue(other.ready(now: start, maxWindow: 5).isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: inbox.root.appendingPathComponent("lost").path),
                       "it can never be opened: dropped")
    }

    func testWithoutAKeyNothingIsWritten() throws {
        let blind = ShareInbox(root: inbox.root)
        XCTAssertThrowsError(try blind.deposit(parsed(), secret: false, expiry: .never, id: "none", now: start, window: 0)) {
            XCTAssertEqual($0 as? ShareSealError, .noKey)
        }
        let names = (try? FileManager.default.contentsOfDirectory(atPath: inbox.root.path)) ?? []
        XCTAssertTrue(names.isEmpty, "no half share, no plaintext")
    }

    func testAShareCopiedUnderAnotherIdDoesNotOpen() throws {
        _ = try inbox.deposit(parsed(), secret: false, expiry: .never, id: "one", now: start, window: 0)
        try FileManager.default.copyItem(at: inbox.root.appendingPathComponent("one"), to: inbox.root.appendingPathComponent("two"))
        XCTAssertEqual(inbox.ready(now: start, maxWindow: 5).map(\.id), ["one"], "the id is part of what was sealed")
    }

    func testAnAlteredSealedFileIsRefusedWhenOpened() throws {
        let file = try sharedFile("bonjour", name: "note.txt")
        let handoff = try inbox.deposit(parsed(files: [file]), secret: false, expiry: .never, id: "bad", now: start, window: 0)
        let claimed = try XCTUnwrap(inbox.claim(handoff))
        let sealed = claimed.appendingPathComponent("files/0")
        var data = try Data(contentsOf: sealed)
        data[0] ^= 1
        try data.write(to: sealed)
        XCTAssertThrowsError(try inbox.parsed(handoff, in: claimed)) {
            XCTAssertEqual($0 as? ShareSealError, .authentication)
        }
    }

    func testTheOpenedFilesLiveInTheClaimedDirectoryAndGoWithIt() throws {
        let file = try sharedFile("bonjour", name: "note.txt")
        let handoff = try inbox.deposit(parsed(files: [file]), secret: false, expiry: .never, id: "go", now: start, window: 0)
        let claimed = try XCTUnwrap(inbox.claim(handoff))
        let back = try inbox.parsed(handoff, in: claimed)
        XCTAssertTrue(back.files[0].url.path.hasPrefix(claimed.path), "never anywhere else")
        XCTAssertEqual(try Data(contentsOf: back.files[0].url), Data("bonjour".utf8))
        // A second look (a failed send, tried again) opens them again.
        XCTAssertEqual(try inbox.parsed(handoff, in: claimed).files.count, 1)
        inbox.finish(claimed)
        XCTAssertFalse(FileManager.default.fileExists(atPath: back.files[0].url.path))
    }

    func testAShareLeftByAnOlderBuildIsStillSent() throws {
        let dir = inbox.root.appendingPathComponent("old1", isDirectory: true)
        try FileManager.default.createDirectory(at: dir.appendingPathComponent("files"), withIntermediateDirectories: true)
        try Data("ancien".utf8).write(to: dir.appendingPathComponent("files/0"))
        let handoff = ShareHandoff(
            version: ShareHandoff.currentVersion, id: "old1", createdAt: start, notBefore: start, title: "Ancien", url: nil,
            plain: "texte", html: nil, markdown: nil, files: [ShareHandoff.File(name: "a.txt", mime: "text/plain", size: 6)],
            looksSecret: false, secret: false, expiry: .never, sealed: nil
        )
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        try encoder.encode(handoff).write(to: dir.appendingPathComponent(ShareInbox.manifestName))
        let ready = inbox.ready(now: start, maxWindow: 5)
        XCTAssertEqual(ready.map(\.id), ["old1"])
        let claimed = try XCTUnwrap(inbox.claim(ready[0]))
        let back = try inbox.parsed(ready[0], in: claimed)
        XCTAssertEqual(back.title, "Ancien")
        XCTAssertEqual(try Data(contentsOf: back.files[0].url), Data("ancien".utf8))
    }
}
