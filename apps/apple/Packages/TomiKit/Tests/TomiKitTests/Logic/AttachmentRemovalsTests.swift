import XCTest
@testable import TomiKit

// Port of apps/android/.../ui/feed/AttachmentRemovalsTest.kt (what the UI offers around the removal of ONE attachment).
final class AttachmentRemovalsTests: XCTestCase {
    private let source = DeviceRef(id: "pc", name: "Desktop")
    private let now = Date(timeIntervalSince1970: 1_793_872_800)
    private let png = Attachment(name: "a.png", mime: "image/png", size: 10)
    private let pdf = Attachment(name: "c.pdf", mime: "application/pdf", size: 10)

    private func item(_ attachments: [Attachment], deliveries: [DeviceDelivery] = []) -> TomiItem {
        .readable(id: "i", sourceDevice: source, createdAt: now, attachments: attachments, deliveries: deliveries)
    }

    private func delivery(_ state: DeliveryState) -> DeviceDelivery {
        DeviceDelivery(deviceId: "d", deviceName: "Phone", state: state)
    }

    func testAnItemStillQueuedOffersNoRemoval() {
        XCTAssertTrue(AttachmentRemovals.isQueued(item([png], deliveries: [delivery(.queued), delivery(.queued)])))
        XCTAssertFalse(AttachmentRemovals.canRemove(item([png, pdf], deliveries: [delivery(.queued)])))
        // One delivery out of the device is enough: the item has left.
        XCTAssertFalse(AttachmentRemovals.isQueued(item([png], deliveries: [delivery(.queued), delivery(.sent)])))
        XCTAssertTrue(AttachmentRemovals.canRemove(item([png], deliveries: [delivery(.sent)])))
        // A received item has no delivery at all.
        XCTAssertTrue(AttachmentRemovals.canRemove(item([png])))
    }

    func testNothingToRemoveWithoutAttachments() {
        XCTAssertFalse(AttachmentRemovals.canRemove(item([])))
    }

    func testTheRowsOfAFileBatchOfferTheRemovalFromTwoAttachmentsOn() {
        XCTAssertFalse(AttachmentRemovals.canRemoveFromBatch(item([png])))
        XCTAssertTrue(AttachmentRemovals.canRemoveFromBatch(item([png, pdf])))
        XCTAssertFalse(AttachmentRemovals.canRemoveFromBatch(item([png, pdf], deliveries: [delivery(.queued)])))
    }

    func testTheMessageFollowsWhatTheCoreDid() {
        XCTAssertEqual(AttachmentRemovals.feedbackKey(.itemDeleted, image: true, everywhere: false), "snackbar_deleted_local")
        XCTAssertEqual(AttachmentRemovals.feedbackKey(.itemDeleted, image: false, everywhere: true), "snackbar_deleted_everywhere")
        XCTAssertEqual(AttachmentRemovals.feedbackKey(.attachmentRemoved, image: true, everywhere: false), "snackbar_image_deleted_local")
        XCTAssertEqual(AttachmentRemovals.feedbackKey(.attachmentRemoved, image: true, everywhere: true), "snackbar_image_deleted_everywhere")
        XCTAssertEqual(AttachmentRemovals.feedbackKey(.attachmentRemoved, image: false, everywhere: false), "snackbar_file_deleted_local")
        XCTAssertEqual(AttachmentRemovals.feedbackKey(.attachmentRemoved, image: false, everywhere: true), "snackbar_file_deleted_everywhere")
    }
}
