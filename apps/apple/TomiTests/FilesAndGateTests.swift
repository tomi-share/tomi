import Foundation
import TomiKit
import XCTest
@testable import Tomi

/// The gate of the hidden items, and what leaves Tomi as a file: its name, its single clear copy.
@MainActor
final class FilesAndGateTests: XCTestCase {
    private final class Prompts {
        var strong = 0
        var normal = 0
    }

    // MARK: Authentication

    func testAStrongAuthenticationIsAskedEveryTimeAndOpensNoWindow() async {
        let prompts = Prompts()
        let gate = BiometricGate(
            evaluateStrong: { _ in prompts.strong += 1; return .success },
            evaluate: { _ in prompts.normal += 1; return .success }
        )
        let first = await gate.authenticateStrong(reason: "retirer")
        XCTAssertEqual(first, .success)
        XCTAssertFalse(gate.window.isValid, "a destructive action leaves no 2 minutes behind it")
        _ = await gate.authenticateStrong(reason: "retirer")
        XCTAssertEqual(prompts.strong, 2, "asked again, never remembered")
        XCTAssertEqual(prompts.normal, 0)
    }

    func testAnAuthenticationToShowHoldsUntilTheWindowIsClosed() async {
        let prompts = Prompts()
        let gate = BiometricGate(
            evaluateStrong: { _ in prompts.strong += 1; return .success },
            evaluate: { _ in prompts.normal += 1; return .success }
        )
        let shown = await gate.authenticate(reason: "afficher")
        XCTAssertEqual(shown, .success)
        XCTAssertTrue(gate.window.isValid)
        _ = await gate.authenticate(reason: "afficher")
        XCTAssertEqual(prompts.normal, 1, "inside the window: not asked again")
        // The device locks, the app leaves the screen: the window closes.
        gate.window.invalidate()
        _ = await gate.authenticate(reason: "afficher")
        XCTAssertEqual(prompts.normal, 2)
        // An open window never spares the strong authentication.
        _ = await gate.authenticateStrong(reason: "retirer")
        XCTAssertEqual(prompts.strong, 1)
    }

    func testARefusalOpensNoWindow() async {
        let gate = BiometricGate(evaluateStrong: { _ in .failed }, evaluate: { _ in .cancelled })
        let outcome = await gate.authenticate(reason: "afficher")
        XCTAssertEqual(outcome, .cancelled)
        XCTAssertFalse(gate.window.isValid)
    }

    // MARK: The name of a file handed to another app

    private func name(_ raw: String) -> String {
        FeedViewModel.fileName(Attachment(name: raw, mime: "application/pdf", size: 1))
    }

    func testAFileLeavesUnderItsLastNameOnly() {
        XCTAssertEqual(name("rapport.pdf"), "rapport.pdf")
        XCTAssertEqual(name("../../etc/passwd"), "passwd", "never a path")
        XCTAssertEqual(name("C:\\Users\\moi\\notes.txt"), "notes.txt")
        XCTAssertEqual(name("a:b\u{7}c.txt"), "abc.txt", "neither a colon nor a control character")
        XCTAssertEqual(name(".env"), "env", "never a hidden file")
        XCTAssertEqual(name(".."), "fichier")
        XCTAssertEqual(name(""), "fichier")
        XCTAssertEqual(name("   "), "fichier")
        XCTAssertEqual(name(String(repeating: "a", count: 300)).count, 120)
    }

    // MARK: One clear copy

    func testAFileIsShownThroughALinkNotASecondCopyAndTheLinkIsSwept() throws {
        let source = FileManager.default.temporaryDirectory.appendingPathComponent("cache-\(UUID().uuidString)")
        try Data("contenu".utf8).write(to: source)
        defer { try? FileManager.default.removeItem(at: source) }

        let link = try XCTUnwrap(FeedViewModel.viewLink(source, name: "rapport.pdf"))
        XCTAssertEqual(link.lastPathComponent, "rapport.pdf", "the reader takes the type from the name")
        XCTAssertTrue(link.deletingLastPathComponent().lastPathComponent.hasPrefix("view-"))
        let links = try FileManager.default.attributesOfItem(atPath: source.path)[.referenceCount] as? Int
        XCTAssertEqual(links, 2, "a hard link: the same bytes, no second clear copy")

        // At start, in the background, one minute after the last use on the Mac.
        FeedViewModel.sweepShareCopies()
        XCTAssertFalse(FileManager.default.fileExists(atPath: link.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: link.deletingLastPathComponent().path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: source.path), "the cache of the core is the core's")
    }

    func testAMissingFileGivesNoLinkAndLeavesNoFolder() {
        func viewFolders() -> Set<String> {
            let names = (try? FileManager.default.contentsOfDirectory(atPath: FileManager.default.temporaryDirectory.path)) ?? []
            return Set(names.filter { $0.hasPrefix("view-") })
        }
        let before = viewFolders()
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent("absent-\(UUID().uuidString)")
        XCTAssertNil(FeedViewModel.viewLink(missing, name: "a.pdf"))
        XCTAssertEqual(viewFolders(), before, "the folder made for the link is taken back")
    }

    // MARK: Preferences

    func testTheDisplayChoicesAreRemembered() throws {
        let suite = "prefs-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let prefs = UserPrefs(defaults: defaults)
        XCTAssertNil(prefs.preferredRenderMode, "nothing chosen yet")
        XCTAssertFalse(prefs.codeWrapEnabled)
        prefs.preferredRenderMode = .source
        prefs.codeWrapEnabled = true
        // Another screen, another run: the same suite.
        let again = UserPrefs(defaults: defaults)
        XCTAssertEqual(again.preferredRenderMode, .source)
        XCTAssertTrue(again.codeWrapEnabled)
        again.preferredRenderMode = .preview
        XCTAssertEqual(UserPrefs(defaults: defaults).preferredRenderMode, .preview)
    }
}
