import Foundation
import ImageIO
import TomiKit
import TomiRuntime
import UniformTypeIdentifiers
import XCTest
@testable import Tomi

@MainActor
private final class FakeScheduler: SendScheduling {
    var scheduled: [(String, UndoDeadline)] = []
    var ensured: [String] = []
    var cancelled: [String] = []

    func schedule(itemId: String, deadline: UndoDeadline) { scheduled.append((itemId, deadline)) }
    func ensureScheduled(itemId: String, createdAt: Date) { ensured.append(itemId) }
    func cancel(itemId: String) { cancelled.append(itemId) }
}

private final class FakeStaging: AttachmentStaging, @unchecked Sendable {
    var staged: [String] = []
    var cleaned: [String] = []
    var skipped = 0

    func stage(draftId: String, files: [SharedFile]) async -> StagedAttachments {
        staged.append(draftId)
        let attachments = files.map { DraftAttachment(path: $0.url.path, name: $0.name, mime: $0.mime, size: $0.size) }
        return StagedAttachments(attachments: attachments, skipped: skipped)
    }

    func cleanup(draftId: String) { cleaned.append(draftId) }
}

private struct Failure: Error {}

/// The send pipeline: coordinator (outbox, undo window, cancel), in-app scheduler (wait, dispatch, retry), staging of
/// the files (limits, names, thumbnails).
@MainActor
final class SendCoordinatorTests: XCTestCase {
    private var core: PreviewCore!
    private var scheduler: FakeScheduler!
    private var staging: FakeStaging!
    private var clock: Int64 = 1_000

    private func makeCoordinator() -> SendCoordinator {
        core = PreviewCore(items: PreviewFixtures.all, localDevice: PreviewFixtures.phone)
        scheduler = FakeScheduler()
        staging = FakeStaging()
        return SendCoordinator(
            core: core, scheduler: scheduler, staging: staging,
            now: { Date(timeIntervalSince1970: 1_000) }, monotonicMs: { [unowned self] in clock }, newId: { "draft-1" }
        )
    }

    private func parsed(_ text: String, files: [SharedFile] = []) -> ParsedShare {
        SharePayloadParser.parse(ShareInput(text: text, files: files))
    }

    func testASendGoesToTheOutboxAndLeavesAfterTheUndoWindow() async throws {
        let coordinator = makeCoordinator()
        let sent = try await coordinator.submit(parsed("Bonjour"), secret: false, expiry: .day1)
        XCTAssertEqual(core.enqueued.map(\.id), ["draft-1"])
        XCTAssertEqual(core.enqueued.first?.expiresAt, Date(timeIntervalSince1970: 1_000 + 86_400))
        XCTAssertEqual(scheduler.scheduled.first?.0, "draft-1")
        XCTAssertEqual(scheduler.scheduled.first?.1, UndoDeadline(atElapsedMs: 4_000, windowMs: 3_000))
        XCTAssertEqual(sent.deadline, UndoDeadline(atElapsedMs: 4_000, windowMs: 3_000))
        XCTAssertEqual(staging.cleaned, ["draft-1"])
    }

    func testAnExpiryNotAllowedFallsBackToNever() async throws {
        let coordinator = makeCoordinator()
        _ = try await coordinator.submit(parsed("Bonjour"), secret: false, expiry: .afterFirstRead)
        XCTAssertNil(core.enqueued.first?.expiresAt)
        XCTAssertEqual(core.enqueued.first?.readPolicy, .normal)
    }

    func testTheCopiesGoEvenWhenTheCoreRefuses() async {
        let coordinator = makeCoordinator()
        core.enqueueError = Failure()
        do {
            _ = try await coordinator.submit(parsed("Bonjour"), secret: false, expiry: .never)
            XCTFail("expected a failure")
        } catch {}
        XCTAssertEqual(staging.cleaned, ["draft-1"])
        XCTAssertTrue(scheduler.scheduled.isEmpty)
    }

    func testUndoStopsTheDispatchOnlyWhenTheCoreCancelled() async throws {
        let coordinator = makeCoordinator()
        let sent = try await coordinator.submit(parsed("Bonjour"), secret: false, expiry: .never)
        let outcome = try await coordinator.cancel(sent)
        XCTAssertEqual(outcome, .cancelled)
        XCTAssertEqual(scheduler.cancelled, ["draft-1"])

        core.cancelResult = false
        let late = try await coordinator.cancel(sent)
        XCTAssertEqual(late, .tooLate)
        XCTAssertEqual(scheduler.cancelled, ["draft-1"])
    }

    func testQueuedItemsOfThisDeviceAreResumed() {
        let coordinator = makeCoordinator()
        coordinator.resumeQueued(localDeviceId: PreviewFixtures.phone.id)
        XCTAssertEqual(scheduler.ensured, [PreviewFixtures.outgoingQueued.id])
        coordinator.resumeQueued(localDeviceId: nil)
        XCTAssertEqual(scheduler.ensured.count, 1)
    }
}

@MainActor
final class InAppSendSchedulerTests: XCTestCase {
    private var core: PreviewCore!
    private var sleeps: [Duration] = []
    private var now: Int64 = 0
    private var keptAlive = 0
    private var released = 0

    private func makeScheduler() -> InAppSendScheduler {
        core = PreviewCore(items: [], localDevice: PreviewFixtures.phone)
        return InAppSendScheduler(
            core: core, monotonicMs: { [unowned self] in now }, now: { Date(timeIntervalSince1970: 100) },
            sleep: { [unowned self] duration in
                sleeps.append(duration)
                await Task.yield()
            },
            keepAlive: { [unowned self] in
                keptAlive += 1
                return { [unowned self] in released += 1 }
            }
        )
    }

    private func settle() async {
        for _ in 0..<20 { await Task.yield() }
    }

    func testTheItemLeavesOnceTheWindowIsOver() async {
        let scheduler = makeScheduler()
        scheduler.schedule(itemId: "a", deadline: UndoDeadline(atElapsedMs: 3_000, windowMs: 3_000))
        await settle()
        XCTAssertEqual(sleeps.first, .milliseconds(3_000))
        XCTAssertEqual(core.calls.filter { $0.hasPrefix("dispatch") }, ["dispatch:a"])
        XCTAssertTrue(scheduler.pending.isEmpty)
        XCTAssertEqual(keptAlive, 1)
        XCTAssertEqual(released, 1, "the background time is given back")
    }

    func testAResumedItemStillWaitsForTheEndOfItsWindow() async {
        let scheduler = makeScheduler()
        // Created 2 s before "now" (100 s): 1 s of the window is left.
        scheduler.ensureScheduled(itemId: "a", createdAt: Date(timeIntervalSince1970: 98))
        await settle()
        XCTAssertEqual(sleeps.first, .milliseconds(1_000))
        scheduler.ensureScheduled(itemId: "b", createdAt: Date(timeIntervalSince1970: 10))
        await settle()
        XCTAssertEqual(sleeps.filter { $0 == .milliseconds(1_000) }.count, 1, "an old item leaves at once")
    }

    func testRetryLaterBacksOffThenTriesAgain() async {
        let scheduler = makeScheduler()
        core.dispatchResults = [.retryLater, .retryLater, .sent]
        scheduler.ensureScheduled(itemId: "a", createdAt: Date(timeIntervalSince1970: 0))
        await settle()
        XCTAssertEqual(core.calls.filter { $0.hasPrefix("dispatch") }.count, 3)
        XCTAssertEqual(sleeps, [.seconds(15), .seconds(30)])
    }

    func testCancelStopsTheDispatchAndEnsureDoesNotDuplicate() async {
        let scheduler = makeScheduler()
        scheduler.schedule(itemId: "a", deadline: UndoDeadline(atElapsedMs: 3_000, windowMs: 3_000))
        scheduler.ensureScheduled(itemId: "a", createdAt: Date(timeIntervalSince1970: 0))
        XCTAssertEqual(scheduler.pending, ["a"])
        scheduler.cancel(itemId: "a")
        await settle()
        XCTAssertTrue(core.calls.filter { $0.hasPrefix("dispatch") }.isEmpty)
        XCTAssertTrue(scheduler.pending.isEmpty)
    }
}

final class FileAttachmentStagingTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("staging-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func file(_ name: String, bytes: Int, mime: String = "text/plain", declared: Int64? = nil) throws -> SharedFile {
        let url = root.appendingPathComponent("src-\(UUID().uuidString)")
        try Data(repeating: 65, count: bytes).write(to: url)
        return SharedFile(url: url, name: name, mime: mime, size: declared ?? Int64(bytes))
    }

    func testFilesAreCopiedWithCleanNames() async throws {
        let staging = FileAttachmentStaging(root: root.appendingPathComponent("out"))
        let staged = await staging.stage(draftId: "d", files: [try file("../../etc/passwd", bytes: 10), try file("note:\u{0007}.txt", bytes: 3)])
        XCTAssertEqual(staged.skipped, 0)
        XCTAssertEqual(staged.attachments.map(\.name), ["passwd", "note.txt"])
        XCTAssertEqual(staged.attachments.map(\.size), [10, 3])
        XCTAssertTrue(staged.attachments.allSatisfy { $0.path.hasPrefix(root.appendingPathComponent("out/d").path) })
        staging.cleanup(draftId: "d")
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("out/d").path))
    }

    func testAFileOverTheLimitIsSkipped() async throws {
        let staging = FileAttachmentStaging(root: root.appendingPathComponent("out"))
        let big = try file("big.bin", bytes: 4, declared: ShareLimits.maxFileBytes + 1)
        let staged = await staging.stage(draftId: "d", files: [big, try file("ok.txt", bytes: 4)])
        XCTAssertEqual(staged.skipped, 1)
        XCTAssertEqual(staged.attachments.map(\.name), ["ok.txt"])
    }

    func testAnUnreadableFileIsSkipped() async throws {
        let staging = FileAttachmentStaging(root: root.appendingPathComponent("out"))
        let missing = SharedFile(url: root.appendingPathComponent("missing"), name: "missing.txt", mime: "text/plain", size: 4)
        let staged = await staging.stage(draftId: "d", files: [missing])
        XCTAssertEqual(staged.skipped, 1)
        XCTAssertTrue(staged.attachments.isEmpty)
    }

    /// `DraftThumbnails`, as Android: 600 px wide at most, JPEG for an opaque image.
    func testAnImageGetsAJPEGThumbnailOf600Pixels() async throws {
        let url = root.appendingPathComponent("photo.png")
        let context = CGContext(data: nil, width: 1200, height: 800, bitsPerComponent: 8, bytesPerRow: 0,
                                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
        context.setFillColor(CGColor(red: 0.2, green: 0.4, blue: 0.8, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 1200, height: 800))
        let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(destination, context.makeImage()!, nil)
        XCTAssertTrue(CGImageDestinationFinalize(destination))
        let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize!
        let staging = FileAttachmentStaging(root: root.appendingPathComponent("out"))
        let staged = await staging.stage(draftId: "d", files: [SharedFile(url: url, name: "photo.png", mime: "image/png", size: Int64(size))])
        XCTAssertNil(staged.attachments.first?.thumbnail, "made apart, never for a hidden item")
        let withThumbnails = await staging.thumbnails(staged.attachments)
        let thumbnail = try XCTUnwrap(withThumbnails.first?.thumbnail)
        XCTAssertEqual(withThumbnails.first?.thumbnailMime, "image/jpeg")
        let source = try XCTUnwrap(CGImageSourceCreateWithData(thumbnail as CFData, nil))
        let image = try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil))
        XCTAssertEqual(image.width, 600)
        XCTAssertEqual(image.height, 400)
        XCTAssertLessThanOrEqual(thumbnail.count, ThumbnailSize.maxBytes)
    }
}

/// The send screen state: paste, hidden content, expiry, send, undo.
@MainActor
final class SendViewModelTests: XCTestCase {
    private var core: PreviewCore!
    private var root: URL!

    private func makeModel() -> SendViewModel {
        core = PreviewCore(items: [], localDevice: PreviewFixtures.phone)
        root = FileManager.default.temporaryDirectory.appendingPathComponent("send-\(UUID().uuidString)", isDirectory: true)
        let picked = PickedFiles(root: root.appendingPathComponent("picked"))
        let coordinator = SendCoordinator(
            core: core, scheduler: FakeScheduler(), staging: FakeStaging(), newId: { "draft" }
        )
        addTeardownBlock { [root] in try? FileManager.default.removeItem(at: root!) }
        return SendViewModel(core: core, coordinator: coordinator, picked: picked)
    }

    func testWhatLooksLikeAPasswordIsHiddenButCanBeShown() {
        let model = makeModel()
        model.onPasted(plain: "Kx9#mP2$vL7q!", html: nil, concealed: false)
        XCTAssertTrue(model.secret)
        XCTAssertEqual(model.secretHint, .heuristic)
        XCTAssertFalse(model.secretLocked)
        model.onSecretChange(false)
        XCTAssertFalse(model.secret)
        XCTAssertEqual(model.secretHint, SecretHint.none)
    }

    func testWhatAPasswordManagerCopiedStaysHidden() {
        let model = makeModel()
        model.onPasted(plain: "correct horse", html: nil, concealed: true)
        XCTAssertTrue(model.secretLocked)
        XCTAssertEqual(model.secretHint, .sourceSensitive)
        model.onSecretChange(false)
        XCTAssertTrue(model.secret)
    }

    func testAnEmptyPasteSaysSo() {
        let model = makeModel()
        model.onPasted(plain: nil, html: nil, concealed: false)
        XCTAssertEqual(model.notice, "notice_paste_empty")
        XCTAssertTrue(model.text.isEmpty)
    }

    func testAPasteIsAddedOnANewLine() {
        let model = makeModel()
        model.onTextChange("Bonjour")
        model.onPasted(plain: "https://relay.example.org", html: nil, concealed: false)
        XCTAssertEqual(model.text, "Bonjour\nhttps://relay.example.org")
    }

    func testAfterFirstReadIsDroppedWithAFile() {
        let model = makeModel()
        model.onSecretChange(true)
        model.onExpiryChange(.afterFirstRead)
        XCTAssertEqual(model.expiry, .afterFirstRead)
        model.onAttach([SharedFile(url: URL(fileURLWithPath: "/tmp/x/a.txt"), name: "a.txt", mime: "text/plain", size: 1)])
        XCTAssertEqual(model.expiry, .never)
        XCTAssertFalse(model.expiryOptions.contains(.afterFirstRead))
    }

    func testASendClearsTheFormAndUndoGivesItBack() async {
        let model = makeModel()
        model.onTitleChange("Code")
        model.onTextChange("Kx9#mP2$vL7q!")
        model.onSecretChange(true)
        await model.send()
        XCTAssertEqual(core.enqueued.first?.secret, true)
        XCTAssertNotNil(model.submission)
        XCTAssertTrue(model.text.isEmpty)
        XCTAssertFalse(model.secret)

        await model.undo()
        XCTAssertNil(model.submission)
        XCTAssertEqual(model.title, "Code")
        XCTAssertEqual(model.text, "Kx9#mP2$vL7q!")
        XCTAssertTrue(model.secret)
    }

    func testASecondUndoDoesNothing() async {
        let model = makeModel()
        model.onTextChange("Bonjour")
        await model.send()
        async let first: Void = model.undo()
        async let second: Void = model.undo()
        _ = await (first, second)
        XCTAssertEqual(core.calls.filter { $0.hasPrefix("cancelIfQueued") }.count, 1)
        XCTAssertEqual(model.text, "Bonjour")
        XCTAssertNil(model.notice)
    }

    func testTheEndOfTheWindowWaitsForARunningUndo() async {
        let model = makeModel()
        model.onTextChange("Bonjour")
        await model.send()
        let undo = Task { await model.undo() }
        await Task.yield()
        model.onBannerTimeout()
        await undo.value
        XCTAssertEqual(model.text, "Bonjour")
    }

    func testTooLateKeepsTheFormEmptyAndSaysSo() async {
        let model = makeModel()
        model.onTextChange("Bonjour")
        await model.send()
        core.cancelResult = false
        await model.undo()
        XCTAssertTrue(model.text.isEmpty)
        XCTAssertEqual(model.notice, "notice_too_late")
    }

    func testAFailedSendKeepsWhatWasTyped() async {
        let model = makeModel()
        core.enqueueError = Failure()
        model.onTextChange("Bonjour")
        await model.send()
        XCTAssertEqual(model.text, "Bonjour")
        XCTAssertNil(model.submission)
        XCTAssertEqual(model.notice, "notice_send_failed")
    }

    func testNothingToSendKeepsTheButtonOff() {
        let model = makeModel()
        XCTAssertFalse(model.canSend)
        model.onTextChange("  \n ")
        XCTAssertFalse(model.canSend)
        model.onTitleChange("Titre")
        XCTAssertTrue(model.canSend)
    }

    func testTheRecipientsAreTheOtherDevices() {
        let model = makeModel()
        XCTAssertEqual(model.recipients, ["PC-Bureau", "PC-Maison"])
    }
}
