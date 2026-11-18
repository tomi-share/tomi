import XCTest
@testable import TomiKit

/// Port of RedactionTest.kt and of the model parts of UnreadableItemTest.kt / ItemLogicTest.kt.
final class ModelTests: XCTestCase {
    private let clear = "Hx7!pQ2-zR9-s3cr3t"
    private let device = DeviceRef(id: "pc", name: "PC-Bureau")
    private let now = Date(timeIntervalSince1970: 1_793_872_800)

    private func assertRedacted(_ printed: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(printed.contains(clear), printed, file: file, line: line)
        XCTAssertFalse(printed.contains("PC-Bureau"), printed, file: file, line: line)
        XCTAssertTrue(printed.contains("redacted"), printed, file: file, line: line)
    }

    private func dumped(_ value: Any) -> String {
        var out = ""
        dump(value, to: &out)
        return out
    }

    func testRichTextPrintsNoRepresentation() {
        let text = RichText(plain: clear, html: "<b>\(clear)</b>", markdown: "**\(clear)**")
        assertRedacted(String(describing: text))
        assertRedacted(String(reflecting: text))
        XCTAssertFalse(dumped(text).contains(clear))
    }

    func testATomiItemPrintsNoContentButKeepsItsIdAndTheSecretFlag() {
        let item = TomiItem.readable(
            id: "item-1", sourceDevice: device, createdAt: now, title: clear,
            text: RichText(plain: clear, markdown: clear), url: "https://example.test/\(clear)",
            attachments: [Attachment(name: "\(clear).txt", mime: "text/plain", size: 3, localPath: "/x/\(clear)")],
            secret: true
        )
        let printed = String(describing: item)
        assertRedacted(printed)
        XCTAssertTrue(printed.contains("item-1"))
        XCTAssertTrue(printed.contains("secret=true"))
        assertRedacted(String(describing: item.content))
        XCTAssertFalse(dumped(item).contains(clear))
        XCTAssertFalse(dumped([item]).contains(clear), "inside a collection too")
    }

    func testEqualityIsUnchangedByTheRedaction() {
        let a = TomiItem.readable(id: "i", sourceDevice: device, createdAt: now, title: clear, secret: true)
        let b = TomiItem.readable(id: "i", sourceDevice: device, createdAt: now, title: clear, secret: true)
        let other = TomiItem.readable(id: "i", sourceDevice: device, createdAt: now, title: "other", secret: true)
        XCTAssertTrue(a == b && a.hashValue == b.hashValue)
        XCTAssertNotEqual(a, other)
    }

    func testDraftsPrintNoContent() {
        let draft = OutgoingDraft(
            id: "d", title: clear, text: RichText(plain: clear), url: clear,
            attachments: [DraftAttachment(path: "/p/\(clear)", name: clear, mime: "text/plain", size: 1)]
        )
        assertRedacted(String(describing: draft))
        assertRedacted(String(describing: draft.attachments[0]))
        XCTAssertFalse(dumped(draft).contains(clear))
    }

    func testTheKindFollowsTheFieldsAndASecretWins() {
        func item(url: String? = nil, text: RichText? = nil, files: [Attachment] = [], secret: Bool = false) -> TomiItem {
            .readable(id: "i", sourceDevice: device, createdAt: now, text: text, url: url, attachments: files, secret: secret)
        }
        let image = Attachment(name: "a.jpg", mime: "image/jpeg", size: 1)
        let pdf = Attachment(name: "a.pdf", mime: "application/pdf", size: 1)
        XCTAssertEqual(item(url: "https://x").kind, .link)
        XCTAssertEqual(item(text: RichText(plain: "a")).kind, .text)
        XCTAssertEqual(item(text: RichText(plain: "a", markdown: "# a")).kind, .richText)
        XCTAssertEqual(item(files: [image, image]).kind, .image)
        XCTAssertEqual(item(files: [image, pdf]).kind, .file)
        XCTAssertEqual(item(url: "https://x", secret: true).kind, .secret)
        let unreadable = TomiItem(id: "u", content: .unreadable(.updateRequired), sourceDevice: device, createdAt: now)
        XCTAssertEqual(unreadable.kind, .unreadable)
        XCTAssertNil(unreadable.title)
        XCTAssertTrue(unreadable.attachments.isEmpty)
        XCTAssertFalse(unreadable.secret)
    }

    func testOpeningMarksReadOnlyAReadableUnreadReceivedNonSecretItem() {
        let received = TomiItem.readable(id: "i", sourceDevice: device, createdAt: now, text: RichText(plain: "a"))
        XCTAssertTrue(received.shouldMarkReadWhenOpened(localDeviceId: "me"))
        XCTAssertFalse(received.shouldMarkReadWhenOpened(localDeviceId: "pc"), "sent from here")
        var read = received
        read.read = true
        XCTAssertFalse(read.shouldMarkReadWhenOpened(localDeviceId: "me"))
        let secret = TomiItem.readable(id: "s", sourceDevice: device, createdAt: now, text: RichText(plain: "a"), secret: true)
        XCTAssertFalse(secret.shouldMarkReadWhenOpened(localDeviceId: "me"))
        let unreadable = TomiItem(id: "u", content: .unreadable(.decryptionFailed), sourceDevice: device, createdAt: now)
        XCTAssertFalse(unreadable.shouldMarkReadWhenOpened(localDeviceId: "me"))
    }

    func testRichTextDerivesItsSourceFormat() {
        XCTAssertEqual(RichText(plain: "a").sourceFormat, .plain)
        XCTAssertEqual(RichText(plain: "a", html: "<p>a</p>").sourceFormat, .html)
        XCTAssertEqual(RichText(plain: "a", html: "<p>a</p>", markdown: "a").sourceFormat, .markdown)
        XCTAssertEqual(RichText(plain: "a", html: "<p>a</p>", sourceFormat: .plain).sourceFormat, .plain)
    }

    func testDeliveryStatusSummary() {
        func sent(_ states: [DeliveryState]) -> TomiItem {
            .readable(
                id: "i", sourceDevice: DeviceRef(id: "me", name: "iPhone"), createdAt: now,
                deliveries: states.enumerated().map { DeviceDelivery(deviceId: "d\($0)", deviceName: "D\($0)", state: $1) }
            )
        }
        let received = TomiItem.readable(id: "r", sourceDevice: device, createdAt: now)
        XCTAssertEqual(DeliveryStatus.of(received, localDeviceId: "me"), .received)
        XCTAssertEqual(DeliveryStatus.of(sent([]), localDeviceId: "me"), .sent)
        XCTAssertEqual(DeliveryStatus.of(sent([.queued, .sent]), localDeviceId: "me"), .queued)
        XCTAssertEqual(DeliveryStatus.of(sent([.failed, .queued]), localDeviceId: "me"), .failed)
        XCTAssertEqual(DeliveryStatus.of(sent([.delivered, .sent]), localDeviceId: "me"), .delivered(delivered: 1, total: 2))
        XCTAssertEqual(
            DeliveryStatus.of(sent([.delivered, .expiredBeforeDelivery]), localDeviceId: "me"),
            .expiredBeforeDelivery(delivered: 1, deviceNames: ["D1"])
        )
        XCTAssertEqual(DeliveryStatus.of(sent([.sent, .sent]), localDeviceId: "me"), .sent)
    }

    func testDevicePlatformFromTheCore() {
        XCTAssertEqual(DevicePlatform(core: "ios"), .ios)
        XCTAssertEqual(DevicePlatform(core: "windows"), .windows)
        XCTAssertEqual(DevicePlatform(core: "unknown"), .other)
    }
}
