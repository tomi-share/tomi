import XCTest
@testable import TomiKit

// Port of the notification cases of apps/android/.../ui/ItemLogicTest.kt (tap action per kind, batching within 10 s).
final class NotificationLogicTests: XCTestCase {
    private let source = DeviceRef(id: "pc", name: "PC-Bureau")
    private let now = Date(timeIntervalSince1970: 1_793_872_800)

    private func item(text: RichText? = nil, url: String? = nil, attachments: [Attachment] = [], secret: Bool = false) -> TomiItem {
        .readable(id: "1", sourceDevice: source, createdAt: now, text: text, url: url, attachments: attachments, secret: secret)
    }

    func testNotificationTapDoesTheObviousAction() {
        XCTAssertEqual(NotificationTapAction.forItem(item(url: "https://a.example")), .openLink)
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: "court"))), .copyText)
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: String(repeating: "x", count: 200)))), .openCard)
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: "deux\nlignes"))), .openCard)
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: "a", html: "<b>a</b>"))), .openCard)
        XCTAssertEqual(NotificationTapAction.forItem(item(attachments: [Attachment(name: "a.jpg", mime: "image/jpeg", size: 1)])), .previewImage)
        XCTAssertEqual(NotificationTapAction.forItem(item(attachments: [Attachment(name: "a.pdf", mime: "application/pdf", size: 1)])), .openFile)
        // A code file is read in Tomi's own code screen: the notification opens the card instead of another app.
        XCTAssertEqual(NotificationTapAction.forItem(item(attachments: [Attachment(name: "main.rs", mime: "text/x-rust", size: 1)])), .openCard)
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: "pw"), secret: true)), .authenticateAndCopy)
    }

    /// Added for Swift: "\r\n" is one Character that is not "\n"; it still counts as a line break.
    func testShortTextLimitsAndCrLfLineBreak() {
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: String(repeating: "x", count: 140)))), .copyText)
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: String(repeating: "x", count: 141)))), .openCard)
        XCTAssertEqual(NotificationTapAction.forItem(item(text: RichText(plain: "a\r\nb"))), .openCard)
    }

    func testArrivalsLessThanTenSecondsApartAreGrouped() {
        let batcher = NotificationBatcher()
        XCTAssertEqual(batcher.onArrival(itemId: "a", nowMs: 0), .single)
        XCTAssertEqual(batcher.onArrival(itemId: "b", nowMs: 9_000), .summary(itemIds: ["a", "b"]))
        XCTAssertEqual(batcher.onArrival(itemId: "c", nowMs: 18_000), .summary(itemIds: ["a", "b", "c"]))
        // A quiet period of 10 s or more starts a new batch.
        XCTAssertEqual(batcher.onArrival(itemId: "d", nowMs: 28_000), .single)
    }
}

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/notify/IncomingNotificationsSummaryTest.kt and the content rules of
// IncomingNotifications.kt.
final class IncomingNotifierTests: XCTestCase {
    private let office = DeviceRef(id: "pc", name: "PC-Bureau")
    private let phone = DeviceRef(id: "phone", name: "iPhone")

    private func item(_ id: String, from: DeviceRef? = nil, read: Bool = false, secret: Bool = false) -> TomiItem {
        TomiItem.readable(id: id, sourceDevice: from ?? office, createdAt: Date(), text: RichText(plain: "texte \(id)"),
                          secret: secret, read: read)
    }

    func testTheExistingHistoryNotifiesNothing() {
        var state = IncomingNotifierState()
        XCTAssertEqual(state.update(items: [item("a"), item("b")], localDeviceId: "phone", appVisible: false).show, [])
    }

    func testANewUnreadItemFromAnotherDeviceIsShownOldestFirst() {
        var state = IncomingNotifierState()
        _ = state.update(items: [item("a")], localDeviceId: "phone", appVisible: false)
        let update = state.update(
            items: [item("c"), item("b"), item("mine", from: phone), item("seen", read: true), item("a")],
            localDeviceId: "phone", appVisible: false
        )
        XCTAssertEqual(update.show.map(\.id), ["b", "c"])
    }

    func testNothingIsShownWhileTheAppIsOnScreen() {
        var state = IncomingNotifierState()
        _ = state.update(items: [], localDeviceId: "phone", appVisible: true)
        XCTAssertEqual(state.update(items: [item("a")], localDeviceId: "phone", appVisible: true).show, [])
    }

    func testTheHistoryImportedAfterThePairingIsNotNotifiedButIsKnownAfterwards() {
        var state = IncomingNotifierState()
        _ = state.update(items: [], localDeviceId: "phone", appVisible: false)
        let imported = state.update(items: [item("old1"), item("old2")], localDeviceId: "phone", appVisible: false, importing: true)
        XCTAssertEqual(imported.show, [])
        // Once the grace is over, only what is new since then is shown.
        let later = state.update(items: [item("new"), item("old1"), item("old2")], localDeviceId: "phone", appVisible: false)
        XCTAssertEqual(later.show.map(\.id), ["new"])
    }

    func testANotificationIsTakenDownOnceReadElsewhereOrGone() {
        var state = IncomingNotifierState()
        _ = state.update(items: [], localDeviceId: "phone", appVisible: false)
        _ = state.update(items: [item("a"), item("b")], localDeviceId: "phone", appVisible: false)
        let update = state.update(items: [item("a", read: true)], localDeviceId: "phone", appVisible: false)
        XCTAssertEqual(update.cancel, ["a", "b"])
        XCTAssertEqual(state.update(items: [], localDeviceId: "phone", appVisible: false).cancel, [])
    }

    func testAHiddenItemSaysNothingNotEvenTheDeviceName() {
        let content = IncomingNotificationContent.forItem(item("s", secret: true))
        XCTAssertEqual(content.group, .secrets)
        XCTAssertEqual(content.titleKey, "notif_public_title")
        XCTAssertNil(content.titleArgument)
        XCTAssertEqual(content.body, .key("notif_body_secret"))
        XCTAssertNil(content.secondary, "copying a secret needs the app (biometrics)")
    }

    func testTheOtherKindsSayWhatTheyAreAndWhereFrom() {
        let link = IncomingNotificationContent.forItem(
            TomiItem.readable(id: "l", sourceDevice: office, createdAt: Date(), url: "https://example.org/a")
        )
        XCTAssertEqual(link.titleKey, "notif_title_link")
        XCTAssertEqual(link.titleArgument, "PC-Bureau")
        XCTAssertEqual(link.body, .text("example.org"))
        XCTAssertEqual(link.secondary, .copy)
        let text = IncomingNotificationContent.forItem(item("t"))
        XCTAssertEqual(text.group, .text)
        XCTAssertEqual(text.body, .text("texte t"))
        XCTAssertEqual(text.secondary, .view, "a short text is copied by a tap; the button opens it")
        let long = IncomingNotificationContent.forItem(
            TomiItem.readable(id: "x", sourceDevice: office, createdAt: Date(), text: RichText(plain: String(repeating: "a", count: 300)))
        )
        XCTAssertEqual(long.body, .text(String(repeating: "a", count: 120)))
    }
}

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/notify/PairingGraceTest.kt: the history imported right after
// the pairing is not announced as a burst of notifications.
final class PairingGraceTests: XCTestCase {
    private var now = Date(timeIntervalSince1970: 1)
    private var grace = PairingGrace(grace: 60)

    private func importing(_ paired: Bool) -> Bool { grace.isImporting(paired: paired, now: now) }

    func testADeviceAlreadyPairedAtStartNeverHasAGracePeriod() {
        XCTAssertFalse(importing(true))
        now += 0.001
        XCTAssertFalse(importing(true))
    }

    func testADeviceThatJoinsWhileRunningIsImportingDuringTheGraceThenNot() {
        XCTAssertFalse(importing(false))
        XCTAssertTrue(importing(true))
        now += 59
        XCTAssertTrue(importing(true))
        now += 2
        XCTAssertFalse(importing(true), "a real item that arrives later notifies again")
        now += 0.01
        XCTAssertFalse(importing(true))
    }

    func testLeavingTheSpaceEndsTheGrace() {
        _ = importing(false)
        XCTAssertTrue(importing(true))
        XCTAssertFalse(importing(false))
        XCTAssertFalse(importing(false))
    }
}
