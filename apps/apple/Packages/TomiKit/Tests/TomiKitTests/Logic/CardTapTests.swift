import XCTest
@testable import TomiKit

// Port of apps/android/.../ui/feed/CardTapTest.kt. The clip detection cases are in ClipDetectionTests.swift; the copy
// callback cases (`onCopy`, `RenderMode`) are not ported (see the report).
final class CardTapTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_793_872_800)
    private let source = DeviceRef(id: "pc", name: "PC-Bureau")

    private func item(
        title: String? = nil, text: RichText? = nil, url: String? = nil, attachments: [Attachment] = [], secret: Bool = false
    ) -> TomiItem {
        .readable(id: "1", sourceDevice: source, createdAt: now, title: title, text: text, url: url, attachments: attachments, secret: secret)
    }

    private let png = Attachment(name: "a.png", mime: "image/png", size: 10)
    private let png2 = Attachment(name: "b.png", mime: "image/png", size: 10)
    private let pdf = Attachment(name: "a.pdf", mime: "application/pdf", size: 10)
    private let zip = Attachment(name: "a.zip", mime: "application/zip", size: 10)
    private let rs = Attachment(name: "main.rs", mime: "text/x-rust", size: 10)
    private let js = Attachment(name: "app.js", mime: "text/javascript", size: 10)
    private let kt = Attachment(name: "Notes.kt", mime: "application/octet-stream", size: 10)
    private let cut = CardOverflow(body: true)
    private let titleCut = CardOverflow(title: true)

    private var unreadable: TomiItem {
        TomiItem(id: "u", content: .unreadable(.updateRequired), sourceDevice: source, createdAt: now)
    }

    private func action(_ item: TomiItem, revealed: Bool = false, overflow: CardOverflow = CardOverflow()) -> CardTapAction {
        CardTap.actionFor(item, revealed: revealed, overflow: overflow)
    }

    // MARK: links

    func testTapOnALinkOpensIt() {
        XCTAssertEqual(action(item(url: "https://a.example")), .openLink)
        XCTAssertEqual(action(item(title: "Page", url: "https://a.example")), .openLink)
    }

    func testALongPageTitleDoesNotStopALinkFromOpening() {
        let link = item(title: String(repeating: "Un titre de page vraiment très long ", count: 5), url: "https://a.example")
        XCTAssertEqual(action(link, overflow: titleCut), .openLink)
        XCTAssertFalse(CardTap.showsReadMore(link, revealed: false, overflow: titleCut))
    }

    func testALinkWithACutTextBodyOpensTheDetail() {
        let link = item(text: RichText(plain: String(repeating: "long ", count: 200)), url: "https://a.example")
        XCTAssertEqual(action(link, overflow: cut), .openDetail)
        XCTAssertTrue(CardTap.showsReadMore(link, revealed: false, overflow: cut))
    }

    func testAUrlThatCannotBeOpenedFallsBackToTheDetail() {
        XCTAssertEqual(action(item(url: "ftp://a.example/x")), .openDetail)
        XCTAssertEqual(action(item(url: "javascript:alert(1)")), .openDetail)
        // No "Lire la suite": nothing is cut, the detail is only a fallback.
        XCTAssertFalse(CardTap.showsReadMore(item(url: "ftp://a.example/x"), revealed: false, overflow: CardOverflow()))
    }

    // MARK: hidden items

    func testTapOnAHiddenItemRevealsItAndNeverOpensTheDetail() {
        let secret = item(text: RichText(plain: "Hx7!pQ2"), secret: true)
        XCTAssertEqual(action(secret), .reveal)
        // Nothing is rendered while hidden, so a stale overflow flag must not change anything or show "Lire la suite".
        XCTAssertEqual(action(secret, overflow: cut), .reveal)
        XCTAssertFalse(CardTap.showsReadMore(secret, revealed: false, overflow: cut))
        let secretLink = item(url: "https://a.example", secret: true)
        XCTAssertEqual(action(secretLink), .reveal)
    }

    func testSecondTapOnARevealedItemHidesItAgainUnlessItIsCut() {
        let secret = item(text: RichText(plain: "Hx7!pQ2"), secret: true)
        XCTAssertEqual(action(secret, revealed: true), .hide)
        XCTAssertFalse(CardTap.showsReadMore(secret, revealed: true, overflow: CardOverflow()))
        XCTAssertEqual(action(secret, revealed: true, overflow: cut), .openDetail)
        XCTAssertEqual(action(secret, revealed: true, overflow: titleCut), .openDetail)
        XCTAssertTrue(CardTap.showsReadMore(secret, revealed: true, overflow: cut))
    }

    func testARevealedItemNeverCopiesOrOpensOnTap() {
        // The sensitive copy goes through the menu / the detail toolbar (clipboard flag, 30 s clearing).
        XCTAssertEqual(action(item(url: "https://a.example", secret: true), revealed: true), .hide)
        XCTAssertEqual(action(item(attachments: [pdf], secret: true), revealed: true), .hide)
    }

    // MARK: text

    func testTapOnAShortTextNeverCopiesIt() {
        // The tap only marks the item read (handleTap always does that first); it copies nothing and opens nothing.
        XCTAssertEqual(action(item(text: RichText(plain: "court"))), .none)
        XCTAssertEqual(action(item(title: "Titre", text: RichText(plain: "court"))), .none)
        XCTAssertEqual(action(item(text: RichText(plain: "a", markdown: "**a**"))), .none)
        XCTAssertEqual(action(item(text: RichText(plain: "a", html: "<b>a</b>"))), .none)
        XCTAssertFalse(CardTap.showsReadMore(item(text: RichText(plain: "court")), revealed: false, overflow: CardOverflow()))
    }

    func testNoTapActionCopiesAnyMore() {
        XCTAssertTrue(CardTapAction.allCases.allSatisfy { "\($0)" != "copy" })
    }

    func testATextCardHasAnExplicitCopyButton() {
        XCTAssertTrue(CardTap.showsCopyButton(item(text: RichText(plain: "court"))))
        XCTAssertTrue(CardTap.showsCopyButton(item(text: RichText(plain: "a", markdown: "**a**"))))
        XCTAssertTrue(CardTap.showsCopyButton(item(text: RichText(plain: "a", html: "<b>a</b>"))))
        XCTAssertTrue(CardTap.showsCopyButton(item(title: "Titre", text: RichText(plain: "court"), url: "https://a.example")))
        // A link-only card copies its link (the tap opens it).
        XCTAssertTrue(CardTap.showsCopyButton(item(url: "https://a.example")))
    }

    func testNoCopyButtonWithoutTextNorLinkNorOnAHiddenItem() {
        XCTAssertFalse(CardTap.showsCopyButton(item(title: "Titre")))
        XCTAssertFalse(CardTap.showsCopyButton(item(attachments: [pdf])))
        // A hidden item copies through its menu, behind the biometric gate and the sensitive clipboard.
        XCTAssertFalse(CardTap.showsCopyButton(item(text: RichText(plain: "Hx7!pQ2"), secret: true)))
        XCTAssertFalse(CardTap.showsCopyButton(item(url: "https://a.example", secret: true)))
        XCTAssertFalse(CardTap.showsCopyButton(unreadable))
    }

    func testTapOnACutTextOpensTheDetail() {
        let long = item(text: RichText(plain: String(repeating: "long ", count: 300)))
        XCTAssertEqual(action(long, overflow: cut), .openDetail)
        XCTAssertTrue(CardTap.showsReadMore(long, revealed: false, overflow: cut))
        let rich = item(text: RichText(plain: "a", markdown: String(repeating: "# a\n", count: 100)))
        XCTAssertEqual(action(rich, overflow: cut), .openDetail)
        XCTAssertTrue(CardTap.showsReadMore(rich, revealed: false, overflow: cut))
    }

    func testACutTitleOpensTheDetailForATextItem() {
        let text = item(title: String(repeating: "titre ", count: 50), text: RichText(plain: "court"))
        XCTAssertEqual(action(text, overflow: titleCut), .openDetail)
        XCTAssertTrue(CardTap.showsReadMore(text, revealed: false, overflow: titleCut))
    }

    func testATitleOnlyItemHasNothingToCopy() {
        XCTAssertEqual(action(item(title: "Titre")), .none)
        XCTAssertEqual(action(item(title: String(repeating: "titre ", count: 50)), overflow: titleCut), .openDetail)
    }

    // MARK: files and images

    func testTapOnASingleFileOpensItWithTheSystemViewer() {
        XCTAssertEqual(action(item(attachments: [pdf])), .openAttachment)
        XCTAssertEqual(action(item(attachments: [zip])), .openAttachment)
    }

    func testTapOnASingleCodeFileOpensTheCodeScreen() {
        XCTAssertEqual(action(item(attachments: [rs])), .openCodeFile)
        XCTAssertEqual(action(item(attachments: [kt])), .openCodeFile)
        XCTAssertFalse(CardTap.showsReadMore(item(attachments: [rs]), revealed: false, overflow: CardOverflow()))
    }

    func testTapOnAHiddenItemWithACodeFileKeepsItsHiddenItemMeaning() {
        let secretCode = item(attachments: [rs], secret: true)
        XCTAssertEqual(action(secretCode), .reveal)
        XCTAssertEqual(action(secretCode, revealed: true), .hide)
    }

    func testSeveralCodeFilesOrAMixOpenTheDetailAndEachRowRoutesByItsType() {
        XCTAssertEqual(action(item(attachments: [rs, js])), .openDetail)
        XCTAssertEqual(action(item(attachments: [png, rs])), .openDetail)
        // Row by row: image -> image viewer, code -> code screen, other file -> inert.
        XCTAssertEqual(AttachmentRowAction.of(png, imageOpenable: true, codeOpenable: true), .openImage)
        XCTAssertEqual(AttachmentRowAction.of(rs, imageOpenable: true, codeOpenable: true), .openCodeFile)
        XCTAssertEqual(AttachmentRowAction.of(pdf, imageOpenable: true, codeOpenable: true), .none)
        // A screen that gave no callback for a type leaves that row inert.
        XCTAssertEqual(AttachmentRowAction.of(png, imageOpenable: false, codeOpenable: true), .none)
        XCTAssertEqual(AttachmentRowAction.of(rs, imageOpenable: true, codeOpenable: false), .none)
    }

    func testAnImageNamedLikeCodeIsStillAnImage() {
        let odd = Attachment(name: "diagram.rs", mime: "image/png", size: 10)
        XCTAssertEqual(AttachmentRowAction.of(odd, imageOpenable: true, codeOpenable: true), .openImage)
        XCTAssertEqual(action(item(attachments: [odd])), .openImage)
    }

    func testTapOnASingleImageOpensTheInternalViewer() {
        XCTAssertEqual(action(item(attachments: [png])), .openImage)
        // Whatever the format of the image: the viewer is for every image/* attachment.
        let jpeg = Attachment(name: "a.jpg", mime: "image/jpeg", size: 10)
        XCTAssertEqual(action(item(attachments: [jpeg])), .openImage)
        XCTAssertFalse(CardTap.showsReadMore(item(attachments: [png]), revealed: false, overflow: CardOverflow()))
    }

    func testTapOnAnImageWithACutTextOrTitleOpensTheDetailFirst() {
        let image = item(text: RichText(plain: String(repeating: "long ", count: 300)), attachments: [png])
        XCTAssertEqual(action(image, overflow: cut), .openDetail)
        XCTAssertEqual(action(item(title: "t", attachments: [png]), overflow: titleCut), .openDetail)
    }

    func testTapOnAHiddenItemWithAnImageNeverOpensTheViewerDirectly() {
        // The image of a hidden item is reached only after "Afficher", then the viewer asks the biometric gate itself.
        // The tap on the card keeps its hidden-item meaning.
        let secretImage = item(attachments: [png], secret: true)
        XCTAssertEqual(action(secretImage), .reveal)
        XCTAssertEqual(action(secretImage, revealed: true), .hide)
    }

    func testTapOnSeveralAttachmentsOpensTheDetail() {
        XCTAssertEqual(action(item(attachments: [png, png2])), .openDetail)
        XCTAssertEqual(action(item(attachments: [png, pdf])), .openDetail)
        // Several files is not a truncation of text: no "Lire la suite".
        XCTAssertFalse(CardTap.showsReadMore(item(attachments: [png, png2]), revealed: false, overflow: CardOverflow()))
    }

    func testAFileWithACutTextOpensTheDetail() {
        let file = item(text: RichText(plain: String(repeating: "long ", count: 300)), attachments: [pdf])
        XCTAssertEqual(action(file, overflow: cut), .openDetail)
    }

    // MARK: unreadable

    func testTapOnAnUnreadableItemDoesNothing() {
        XCTAssertEqual(action(unreadable), .none)
        XCTAssertEqual(action(unreadable, overflow: cut), .none)
        XCTAssertFalse(CardTap.showsReadMore(unreadable, revealed: false, overflow: cut))
    }

    // MARK: dispatch

    private final class Recorder {
        var calls = [String]()
        lazy var actions = CardActions(
            onOpenCodeFile: { [unowned self] index in calls.append("code:\(index)") },
            onCardOpened: { [unowned self] in calls.append("opened") },
            onOpenDetail: { [unowned self] in calls.append("detail") },
            onOpenAttachment: { [unowned self] in calls.append("attachment") },
            onOpenImage: { [unowned self] index in calls.append("image:\(index)") },
            onReveal: { [unowned self] in calls.append("reveal") },
            onHide: { [unowned self] in calls.append("hide") },
            onOpenLink: { [unowned self] in calls.append("link") }
        )
    }

    func testEveryTapMarksTheItemOpenedFirstThenDoesItsAction() {
        let expected: [CardTapAction: [String]] = [
            .openLink: ["opened", "link"],
            .reveal: ["opened", "reveal"],
            .hide: ["opened", "hide"],
            .openAttachment: ["opened", "attachment"],
            .openCodeFile: ["opened", "code:0"],
            .openImage: ["opened", "image:0"],
            .openDetail: ["opened", "detail"],
            .none: ["opened"],
        ]
        XCTAssertEqual(Set(CardTapAction.allCases), Set(expected.keys))
        for (tap, calls) in expected {
            let recorder = Recorder()
            recorder.actions.handleTap(tap)
            XCTAssertEqual(recorder.calls, calls, "\(tap)")
        }
    }

    func testTheTapOnATextCardNeverReachesAnyActionButOpened() {
        let recorder = Recorder()
        recorder.actions.handleTap(action(item(text: RichText(plain: "court"))))
        // Marked read, nothing else: no copy, no detail.
        XCTAssertEqual(recorder.calls, ["opened"])
    }
}
