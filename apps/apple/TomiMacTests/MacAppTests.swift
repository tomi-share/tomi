import AppKit
import Carbon.HIToolbox
import CoreImage
import TomiKit
import TomiRuntime
import XCTest
@testable import Tomi

/// The rules of the global shortcut (`MacShortcut`): what may be registered, how it is written.
@MainActor
final class MacShortcutTests: XCTestCase {
    private func press(_ keyCode: Int, _ flags: NSEvent.ModifierFlags) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil,
            characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: UInt16(keyCode)
        )!
    }

    func testTheStandardShortcutIsControlShiftSpace() {
        XCTAssertEqual(MacShortcut(event: press(kVK_Space, [.control, .shift])), .standard)
        XCTAssertEqual(MacShortcut.standard.display, "⌃⇧" + L10n.text("macos_key_space"))
    }

    func testCommandAloneIsRefused() {
        // Command-V as a global shortcut would take the paste away from every app.
        XCTAssertNil(MacShortcut(event: press(kVK_ANSI_V, .command)))
        XCTAssertNil(MacShortcut(event: press(kVK_ANSI_Q, [.command, .shift])))
    }

    func testOptionAloneIsRefused() {
        // Option types characters on a French keyboard.
        XCTAssertNil(MacShortcut(event: press(kVK_ANSI_E, .option)))
        XCTAssertNil(MacShortcut(event: press(kVK_ANSI_E, [.option, .shift])))
    }

    func testAPlainKeyIsRefused() {
        XCTAssertNil(MacShortcut(event: press(kVK_ANSI_A, [])))
        XCTAssertNil(MacShortcut(event: press(kVK_Space, .shift)))
    }

    func testControlOrTwoModifiersAreAccepted() {
        XCTAssertNotNil(MacShortcut(event: press(kVK_ANSI_T, .control)))
        XCTAssertNotNil(MacShortcut(event: press(kVK_ANSI_T, [.command, .option])))
        XCTAssertEqual(MacShortcut(event: press(kVK_ANSI_T, [.command, .option]))?.display.prefix(2), "⌥⌘")
    }

    func testAFunctionKeyMayComeAlone() {
        XCTAssertEqual(MacShortcut(event: press(kVK_F5, []))?.display, "F5")
    }

    func testTwoShortcutsOneOpensTomiTheOtherSendsTheSelection() {
        XCTAssertEqual(MacGlobalShortcut.Kind.open.standard.display, "⌃⇧" + L10n.text("macos_key_space"))
        XCTAssertEqual(MacGlobalShortcut.Kind.sendSelection.standard.display, "⌃⇧V")
        XCTAssertNotEqual(MacGlobalShortcut.Kind.open.standard, MacGlobalShortcut.Kind.sendSelection.standard)
    }

    func testTheSameKeysCannotDoBoth() throws {
        // The two shortcuts of the app hosting the tests. Nothing is changed: only a refusal is asked for (the
        // settings of this Mac are the ones of the app).
        let delegate = try XCTUnwrap(MacAppDelegate.shared)
        let open = try XCTUnwrap(delegate.shortcut)
        let send = try XCTUnwrap(delegate.sendShortcut)
        XCTAssertEqual(open.kind, .open)
        XCTAssertEqual(send.kind, .sendSelection)
        let before = (open.current, send.current)
        XCTAssertFalse(send.set(open.current), "the keys of the other shortcut are refused")
        XCTAssertFalse(open.set(send.current))
        XCTAssertEqual(open.current, before.0)
        XCTAssertEqual(send.current, before.1)
    }
}

/// The life of the Mac app in the container: surfaces used or left, what a hidden item showed.
@MainActor
final class MacLifecycleTests: XCTestCase {
    private var roots: [URL] = []
    private var containers: [AppContainer] = []
    private let savedDelay = AppContainer.clearCopiesDelay

    override func tearDown() async throws {
        AppContainer.clearCopiesDelay = savedDelay
        containers.forEach { ($0.core as? RustTomiCoreAdapter)?.shutdown() }
        containers = []
        roots.forEach { try? FileManager.default.removeItem(at: $0) }
        roots = []
    }

    private func demoContainer() async throws -> AppContainer {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        roots.append(root)
        let paths = CorePaths(root: root)
        try paths.prepare()
        let container = AppContainer(mode: .demo(startPaired: true, assets: nil), paths: { paths })
        containers.append(container)
        await container.start()
        let core = try XCTUnwrap(container.core)
        let deadline = Date().addingTimeInterval(120)
        while !(core.state.loaded && !core.state.items.isEmpty) && Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertTrue(core.state.loaded && !core.state.items.isEmpty, "the demo core did not load its feed")
        return container
    }

    private func wait(_ what: String, _ condition: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        XCTAssertTrue(condition(), what)
    }

    func testASurfaceIsOnScreenOnlyWhileItIsUsed() async throws {
        let container = try await demoContainer()
        XCTAssertFalse(container.surfaceVisible, "launched in the menu bar: nothing on screen")
        container.surfaceVisibilityChanged(true)
        XCTAssertTrue(container.surfaceVisible)
        container.surfaceVisibilityChanged(false)
        XCTAssertFalse(container.surfaceVisible)
    }

    func testLeavingASurfaceClosesTheViewerOfAHiddenItemOnly() async throws {
        let container = try await demoContainer()
        let feed = try XCTUnwrap(container.feed)
        let items = try XCTUnwrap(container.core?.state.items)
        let hidden = try XCTUnwrap(items.first { $0.secret && !$0.attachments.isEmpty })
        let plain = try XCTUnwrap(items.first { !$0.secret && $0.attachments.contains(where: \.isImage) })

        feed.viewerRequest = ViewerRequest(itemId: plain.id, attachmentIndex: 0)
        container.surfaceLeft(closed: false)
        XCTAssertNotNil(feed.viewerRequest, "an ordinary image stays open while the window is only left")

        feed.viewerRequest = ViewerRequest(itemId: hidden.id, attachmentIndex: 0)
        container.surfaceLeft(closed: false)
        XCTAssertNil(feed.viewerRequest, "the image of a hidden item does not stay behind a window that was left")

        feed.viewerRequest = ViewerRequest(itemId: plain.id, attachmentIndex: 0)
        feed.fileRequest = ViewerRequest(itemId: plain.id, attachmentIndex: 0)
        container.surfaceLeft(closed: true)
        XCTAssertNil(feed.viewerRequest, "a closed surface takes every viewer with it")
        XCTAssertNil(feed.fileRequest)
    }

    func testTheClearCopiesGoOneDelayAfterTheLastSurfaceIsLeft() async throws {
        AppContainer.clearCopiesDelay = .milliseconds(150)
        let container = try await demoContainer()
        let feed = try XCTUnwrap(container.feed)
        let item = try XCTUnwrap(container.core?.state.items.first { !$0.secret && !$0.attachments.isEmpty })
        container.gate.window.markAuthenticated()

        container.surfaceVisibilityChanged(true)
        feed.fileRequest = ViewerRequest(itemId: item.id, attachmentIndex: 0)
        container.surfaceVisibilityChanged(false)
        XCTAssertNotNil(feed.fileRequest, "not at once: a file just opened elsewhere must still be there")
        try await wait("the purge runs after the delay") { feed.fileRequest == nil }
        XCTAssertFalse(container.gate.window.isValid, "the 2 minutes of an authentication end with the purge")
    }

    func testComingBackBeforeTheDelayKeepsTheClearCopies() async throws {
        AppContainer.clearCopiesDelay = .milliseconds(200)
        let container = try await demoContainer()
        let feed = try XCTUnwrap(container.feed)
        let item = try XCTUnwrap(container.core?.state.items.first { !$0.secret && !$0.attachments.isEmpty })

        container.surfaceVisibilityChanged(true)
        feed.fileRequest = ViewerRequest(itemId: item.id, attachmentIndex: 0)
        container.surfaceVisibilityChanged(false)
        container.surfaceVisibilityChanged(true)
        try await Task.sleep(for: .milliseconds(450))
        XCTAssertNotNil(feed.fileRequest, "a surface used again cancels the purge")
    }

    func testSomethingDecryptedWithNoSurfaceGetsItsPurgeToo() async throws {
        AppContainer.clearCopiesDelay = .milliseconds(150)
        let container = try await demoContainer()
        let feed = try XCTUnwrap(container.feed)
        let item = try XCTUnwrap(container.core?.state.items.first { !$0.secret && !$0.attachments.isEmpty })
        // No surface was ever used: nothing armed the purge yet.
        feed.fileRequest = ViewerRequest(itemId: item.id, attachmentIndex: 0)
        container.schedulePurge()
        try await wait("the purge runs") { feed.fileRequest == nil }
    }

    func testUnreadCountsWhatWasReceivedAndNotRead() async throws {
        let container = try await demoContainer()
        let core = try XCTUnwrap(container.core)
        let local = try XCTUnwrap(core.state.localDevice?.id)
        let expected = core.state.items.filter { !$0.read && !$0.archived && !$0.isOutgoing(localDeviceId: local) }.count
        XCTAssertEqual(container.unreadCount, expected)
        XCTAssertGreaterThan(expected, 0, "the demo feed has unread items")
        try await core.simulateIncoming(.link)
        let deadline = Date().addingTimeInterval(10)
        while container.unreadCount != expected + 1 && Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertEqual(container.unreadCount, expected + 1)
    }

    func testAHiddenItemIsMaskedAgainSoonerOnTheMac() {
        XCTAssertEqual(FeedViewModel.autoHideSeconds, 15)
    }

    func testTheCoreAnnouncesAMac() {
        XCTAssertEqual(CoreFactory.platform, "macos")
    }
}

/// The camera scan of the Mac reads a QR code with Vision (`QRDecoder`): checked on a generated code, no camera needed.
final class MacQRDecoderTests: XCTestCase {
    private func qrCode(_ text: String, scale: CGFloat = 10) throws -> CGImage {
        let filter = try XCTUnwrap(CIFilter(name: "CIQRCodeGenerator"))
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        let output = try XCTUnwrap(filter.outputImage).transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        // On a white margin, as a code shown on a screen.
        let padded = output.extent.insetBy(dx: -40, dy: -40)
        let white = CIImage(color: .white).cropped(to: padded)
        return try XCTUnwrap(CIContext().createCGImage(output.composited(over: white), from: padded))
    }

    func testThePairingLinkOfAPhoneIsRead() throws {
        let link = "https://relay.example.org/l/pair#4821-crayon-renard"
        XCTAssertEqual(QRDecoder.payload(in: try qrCode(link)), link)
    }

    func testAnImageWithoutACodeGivesNothing() throws {
        let blank = try XCTUnwrap(CIContext().createCGImage(CIImage(color: .white).cropped(to: CGRect(x: 0, y: 0, width: 200, height: 200)), from: CGRect(x: 0, y: 0, width: 200, height: 200)))
        XCTAssertNil(QRDecoder.payload(in: blank))
    }
}

/// The texts shared with the phones have their Mac wording (device variations of the string catalog).
final class MacTextsTests: XCTestCase {
    func testTheMacSaysMacAndTouchID() {
        // Whatever the language of the run: every language names the Mac "Mac" and Touch ID "Touch ID".
        XCTAssertTrue(L10n.text("source_this_phone").contains("Mac"))
        XCTAssertTrue(L10n.text("erase_confirm").contains("Mac"))
        XCTAssertTrue(L10n.text("detail_masked_hint").contains("Touch ID"))
        XCTAssertTrue(L10n.text("unreadable_message").contains("Mac"))
    }
}

/// "Enregistrer" on the Mac (`MacDownloads`): a copy that never replaces a file.
@MainActor
final class MacDownloadsTests: XCTestCase {
    private var folder: URL!

    override func setUp() async throws {
        folder = FileManager.default.temporaryDirectory.appendingPathComponent("mac-downloads-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: folder)
    }

    private func source(_ text: String) throws -> URL {
        let url = folder.appendingPathComponent("source-\(UUID().uuidString)")
        try Data(text.utf8).write(to: url)
        // As the cache of the core hands its files: read-only.
        try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: url.path)
        return url
    }

    func testASaveNeverReplacesAFile() throws {
        let target = folder.appendingPathComponent("out", isDirectory: true)
        let first = try XCTUnwrap(MacDownloads.copy(try source("un"), name: "rapport.pdf", into: target))
        let second = try XCTUnwrap(MacDownloads.copy(try source("deux"), name: "rapport.pdf", into: target))
        let third = try XCTUnwrap(MacDownloads.copy(try source("trois"), name: "rapport.pdf", into: target))
        XCTAssertEqual(first.lastPathComponent, "rapport.pdf")
        XCTAssertEqual(second.lastPathComponent, "rapport (2).pdf")
        XCTAssertEqual(third.lastPathComponent, "rapport (3).pdf")
        XCTAssertEqual(try String(contentsOf: first, encoding: .utf8), "un", "the first file is untouched")
    }

    func testANameWithoutExtensionIsNumberedToo() throws {
        let target = folder.appendingPathComponent("out", isDirectory: true)
        _ = MacDownloads.copy(try source("a"), name: "LISEZMOI", into: target)
        let second = try XCTUnwrap(MacDownloads.copy(try source("b"), name: "LISEZMOI", into: target))
        XCTAssertEqual(second.lastPathComponent, "LISEZMOI (2)")
    }

    func testTheSavedCopyIsTheUsersToEdit() throws {
        let saved = try XCTUnwrap(MacDownloads.copy(try source("texte"), name: "note.txt", into: folder.appendingPathComponent("out")))
        XCTAssertTrue(FileManager.default.isWritableFile(atPath: saved.path), "the read-only flag of the cache does not follow the copy")
    }
}

/// The share path of the Mac, end to end without the sheet of the extension: a share put in the inbox of the app group
/// (what the extension does) is sent by the app that runs in the menu bar (here: the test host, on its demo core).
@MainActor
final class MacShareInboxTests: XCTestCase {
    func testTheAppAndItsExtensionShareTheGroupOfTheTeam() throws {
        let group = try XCTUnwrap(Bundle.main.object(forInfoDictionaryKey: "TomiAppGroup") as? String)
        XCTAssertTrue(group.hasSuffix(".eu.shaffe.tomi"), "the group of the team, not the `group.` form of iOS")
        XCTAssertFalse(group.hasPrefix("."), "the team prefix was resolved at build time")
        XCTAssertEqual(AppGroup.identifier, group)
        XCTAssertNotNil(ShareInbox.shared(), "the app can reach the inbox its extension writes in")
    }

    func testAShareLeftInTheInboxIsSentByTheApp() async throws {
        let container = try XCTUnwrap(MacAppDelegate.shared?.container, "the tests run inside the Mac app")
        let core = try XCTUnwrap(container.core)
        let loaded = Date().addingTimeInterval(120)
        while !(core.state.loaded && core.state.isPaired) && Date() < loaded {
            try await Task.sleep(for: .milliseconds(50))
        }
        // Never on a real space: the share would really be sent.
        try XCTSkipUnless(core.isDemo, "the test host must run on the demo core")
        // Nor while another Tomi runs on this Mac (the person's own, in their space): it watches the same inbox of the
        // app group and would take the share and really send it.
        let running = NSRunningApplication.runningApplications(withBundleIdentifier: Bundle.main.bundleIdentifier ?? "")
        try XCTSkipUnless(running.count <= 1, "another Tomi is running: its inbox is the same")
        // What the extension does: seals to the public key the app published in the group.
        let inbox = try XCTUnwrap(ShareInbox.shared(recipientKey: AppGroup.inboxPublicKey))
        let text = "Partagé depuis une autre app \(UUID().uuidString)"
        let parsed = SharePayloadParser.parse(ShareInput(text: text))
        // No undo window left: the app may send it at once.
        _ = try inbox.deposit(parsed, secret: false, expiry: ExpiryPolicy.defaultChoice, now: Date().addingTimeInterval(-10), window: 0)
        CFNotificationCenterPostNotification(
            CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName(AppGroup.inboxChanged as CFString), nil, nil, true
        )
        let deadline = Date().addingTimeInterval(15)
        func sent() -> Bool { core.state.items.contains { $0.text?.plain == text } }
        while !sent() && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertTrue(sent(), "the share is in the feed of the app, sent from this Mac")
    }
}

/// "Ouvrir" never runs nor installs what another device sent (`MacFileActions.runsOrInstalls`).
@MainActor
final class MacOpenSafetyTests: XCTestCase {
    private var folder: URL!

    override func setUp() async throws {
        folder = FileManager.default.temporaryDirectory.appendingPathComponent("mac-open-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: folder)
    }

    private func file(_ name: String, executable: Bool = false) throws -> URL {
        let url = folder.appendingPathComponent(name)
        try Data("contenu".utf8).write(to: url)
        if executable { try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path) }
        return url
    }

    func testWhatRunsOrInstallsIsNeverOpened() throws {
        for name in ["installer.pkg", "profil.terminal", "outil.jar", "lancer.command", "script.sh", "auto.scpt",
                     "reglage.mobileconfig", "lien.webloc", "image.dmg", "archive.zip", "appli.app", "flux.workflow"] {
            XCTAssertTrue(MacFileActions.runsOrInstalls(try file(name)), "\(name) must be shown in the Finder, not opened")
        }
    }

    func testAProgramWithoutExtensionIsNotOpened() throws {
        XCTAssertTrue(MacFileActions.runsOrInstalls(try file("binaire", executable: true)))
    }

    func testDocumentsAndMediaAreOpened() throws {
        for name in ["rapport.pdf", "photo.jpg", "notes.txt", "tableau.csv", "son.mp3", "film.mp4", "document.docx"] {
            XCTAssertFalse(MacFileActions.runsOrInstalls(try file(name)), "\(name) is a document")
        }
    }
}

/// What only the Mac does with a hidden content: nothing of it leaves by a drag, and its window leaves the captures.
@MainActor
final class MacHiddenContentTests: XCTestCase {
    private func makeModel() -> FeedViewModel {
        FeedViewModel(
            core: PreviewCore(items: PreviewFixtures.all, localDevice: PreviewFixtures.phone), prefs: UserPrefs(),
            gate: BiometricGate { _ in .success }, network: NetworkMonitor(), feedback: FeedbackCenter()
        )
    }

    func testAFileIsDraggedOutUnderItsNameButNeverAHiddenOne() {
        let model = makeModel()
        let provider = model.dragProvider(PreviewFixtures.files, index: 0)
        XCTAssertEqual(provider?.suggestedName, "devis-2026.pdf")
        XCTAssertEqual(provider?.registeredTypeIdentifiers, ["com.adobe.pdf"])
        XCTAssertNil(model.dragProvider(PreviewFixtures.files, index: 9), "no file there")

        let hidden = TomiItem.readable(
            id: "hidden", sourceDevice: PreviewFixtures.office, createdAt: Date(),
            attachments: [Attachment(name: "releve.pdf", mime: "application/pdf", size: 1)], secret: true
        )
        XCTAssertNil(model.dragProvider(hidden, index: 0), "nothing of a hidden item leaves by a drag")
        XCTAssertNil(model.dragProvider(PreviewFixtures.unreadable, index: 0))
    }

    func testAWindowLeavesTheCapturesWhileItShowsSomethingProtected() {
        // Developer builds protect only when asked to (so that the app being developed can be captured).
        // The test runs inside the app: what the developer had set comes back after it.
        let set = UserDefaults.standard.object(forKey: "TomiProtectCaptures")
        UserDefaults.standard.set(true, forKey: "TomiProtectCaptures")
        defer {
            if let set { UserDefaults.standard.set(set, forKey: "TomiProtectCaptures") } else { UserDefaults.standard.removeObject(forKey: "TomiProtectCaptures") }
        }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 10, height: 10), styleMask: [.titled], backing: .buffered, defer: true)
        XCTAssertEqual(window.sharingType, .readOnly)
        // A hidden content shown, and the screen that shows it (two protected views).
        WindowCaptureGuard.retain(window)
        WindowCaptureGuard.retain(window)
        XCTAssertEqual(window.sharingType, .none)
        WindowCaptureGuard.release(window)
        XCTAssertEqual(window.sharingType, .none, "one protected view is left")
        WindowCaptureGuard.release(window)
        XCTAssertEqual(window.sharingType, .readOnly, "back in the captures with the last one")
        // One release too many changes nothing.
        WindowCaptureGuard.release(window)
        XCTAssertEqual(window.sharingType, .readOnly)
    }
}
