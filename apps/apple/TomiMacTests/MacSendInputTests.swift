import AppKit
import TomiKit
import TomiRuntime
import XCTest
@testable import Tomi

/// What the Mac reads from a pasteboard at a paste (`MacSendInput`). On a pasteboard of its own: the general one, the
/// user's, is never touched by a test.
@MainActor
final class MacSendInputTests: XCTestCase {
    private var board: NSPasteboard!
    private var folder: URL!

    override func setUp() async throws {
        board = NSPasteboard(name: NSPasteboard.Name("eu.shaffe.tomi.tests.\(UUID().uuidString)"))
        board.clearContents()
        folder = FileManager.default.temporaryDirectory.appendingPathComponent("mac-send-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        board.releaseGlobally()
        try? FileManager.default.removeItem(at: folder)
    }

    private var concealed: NSPasteboard.PasteboardType { NSPasteboard.PasteboardType(SensitiveClipboard.concealedType) }

    private func file(_ name: String, bytes: Int = 12) throws -> URL {
        let url = folder.appendingPathComponent(name)
        try Data(repeating: 0x41, count: bytes).write(to: url)
        return url
    }

    private func png() -> Data {
        let bitmap = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
            isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
        )!
        return bitmap.representation(using: .png, properties: [:])!
    }

    private func model() -> (SendViewModel, PickedFiles) {
        let core = PreviewCore(items: PreviewFixtures.all, localDevice: PreviewFixtures.phone)
        let coordinator = SendCoordinator(
            core: core, scheduler: InAppSendScheduler(core: core),
            staging: FileAttachmentStaging(root: folder.appendingPathComponent("outgoing"))
        )
        let picked = PickedFiles(root: folder.appendingPathComponent("picked"))
        return (SendViewModel(core: core, coordinator: coordinator, picked: picked), picked)
    }

    // MARK: Reading

    func testATextIsReadWithItsRichVariant() {
        board.setString("Bonjour", forType: .string)
        board.setString("<b>Bonjour</b>", forType: .html)
        let content = MacSendInput.read(board)
        XCTAssertEqual(content.plain, "Bonjour")
        XCTAssertEqual(content.html, "<b>Bonjour</b>")
        XCTAssertFalse(content.concealed)
        XCTAssertNil(content.image)
        XCTAssertTrue(content.files.isEmpty)
        XCTAssertFalse(MacSendInput.holdsFilesOrImage(board))
        XCTAssertFalse(MacSendInput.holdsConcealed(board))
    }

    func testTheMarkOfAPasswordManagerIsSeen() {
        board.setString("hunter2", forType: .string)
        board.setString("", forType: concealed)
        XCTAssertTrue(MacSendInput.read(board).concealed)
        XCTAssertTrue(MacSendInput.holdsConcealed(board))
        XCTAssertFalse(MacSendInput.holdsFilesOrImage(board))
    }

    func testCopiedFilesAreTheContentNotTheirNames() throws {
        let url = try file("rapport.pdf")
        board.writeObjects([url as NSURL])
        board.addTypes([.string], owner: nil)
        board.setString("rapport.pdf", forType: .string)
        let content = MacSendInput.read(board)
        XCTAssertEqual(content.files.map(\.lastPathComponent), ["rapport.pdf"])
        XCTAssertNil(content.plain, "the name of a copied file is not a text to send")
        XCTAssertTrue(MacSendInput.holdsFilesOrImage(board))
    }

    func testAnImageCopiedAloneIsTaken() {
        board.setData(png(), forType: .png)
        XCTAssertNotNil(MacSendInput.read(board).image)
        XCTAssertTrue(MacSendInput.holdsFilesOrImage(board))
    }

    func testAnImageBesideATextIsLeftToTheText() {
        board.setString("Légende", forType: .string)
        board.addTypes([.png], owner: nil)
        board.setData(png(), forType: .png)
        let content = MacSendInput.read(board)
        XCTAssertEqual(content.plain, "Légende")
        XCTAssertNil(content.image)
        XCTAssertFalse(MacSendInput.holdsFilesOrImage(board), "a text with an image goes to the text field")
    }

    func testAnEmptyPasteboardGivesNothing() {
        let content = MacSendInput.read(board)
        XCTAssertNil(content.plain)
        XCTAssertNil(content.image)
        XCTAssertTrue(content.files.isEmpty)
        XCTAssertFalse(MacSendInput.holdsFilesOrImage(board))
    }

    // MARK: Pasting into the draft

    func testAPastedTextFillsTheDraftAndKeepsItsRichVariant() async {
        let (draft, picked) = model()
        board.setString("Bonjour", forType: .string)
        board.setString("<b>Bonjour</b>", forType: .html)
        await MacSendInput.paste(into: draft, picked: picked, board: board)
        XCTAssertEqual(draft.text, "Bonjour")
        XCTAssertFalse(draft.secret)
        XCTAssertTrue(draft.attachments.isEmpty)
    }

    func testAConcealedPasteHidesTheContentAndLocksTheSwitch() async {
        let (draft, picked) = model()
        board.setString("correct horse battery staple", forType: .string)
        board.setString("", forType: concealed)
        await MacSendInput.paste(into: draft, picked: picked, board: board)
        XCTAssertTrue(draft.secret)
        XCTAssertTrue(draft.secretLocked)
        XCTAssertEqual(draft.secretHint, .sourceSensitive)
    }

    func testPastedFilesAreAttachedAndCopiedIntoTheApp() async throws {
        let (draft, picked) = model()
        let urls = [try file("a.txt"), try file("b.txt")]
        board.writeObjects(urls.map { $0 as NSURL })
        await MacSendInput.paste(into: draft, picked: picked, board: board)
        XCTAssertEqual(draft.attachments.map(\.file.name).sorted(), ["a.txt", "b.txt"])
        XCTAssertTrue(draft.attachments.allSatisfy { $0.file.url.path.hasPrefix(picked.root.path) }, "copied at once, not borrowed")
        XCTAssertTrue(draft.text.isEmpty)
    }

    func testAPastedImageBecomesAnAttachment() async {
        let (draft, picked) = model()
        board.setData(png(), forType: .png)
        await MacSendInput.paste(into: draft, picked: picked, board: board)
        XCTAssertEqual(draft.attachments.count, 1)
        XCTAssertEqual(draft.attachments.first?.file.mime, "image/png")
    }

    func testOnlyWhatTheDraftHasRoomForIsCopied() async throws {
        let (draft, picked) = model()
        let urls = try (0..<(SendViewModel.maxAttachments + 5)).map { try file("f\($0).txt") }
        await MacSendInput.attach(urls, to: draft, picked: picked)
        XCTAssertEqual(draft.attachments.count, SendViewModel.maxAttachments)
        let copied = (try? FileManager.default.contentsOfDirectory(atPath: picked.root.path))?.count ?? 0
        XCTAssertEqual(copied, SendViewModel.maxAttachments, "the files past the limit are not even copied")
        XCTAssertEqual(draft.notice, "ios_notice_files_skipped")
    }

    func testAFileOverTheLimitIsSkipped() async throws {
        let (draft, picked) = model()
        let big = try file("gros.bin", bytes: Int(ShareLimits.maxFileBytes) + 1)
        await MacSendInput.attach([big], to: draft, picked: picked)
        XCTAssertTrue(draft.attachments.isEmpty)
        XCTAssertEqual(draft.notice, "ios_notice_files_skipped")
    }
}
