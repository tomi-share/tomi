import XCTest
@testable import TomiKit

// Port of apps/android/.../ui/viewer/ViewerLogicTest.kt.
final class ViewerLogicTests: XCTestCase {
    private let source = DeviceRef(id: "pc", name: "PC-Bureau")
    private let now = Date(timeIntervalSince1970: 1_793_872_800)

    private let png = Attachment(name: "a.png", mime: "image/png", size: 10)
    private let jpg = Attachment(name: "b.jpg", mime: "image/jpeg", size: 10)
    private let pdf = Attachment(name: "c.pdf", mime: "application/pdf", size: 10)

    private func item(_ attachments: [Attachment], secret: Bool = false) -> TomiItem {
        .readable(id: "i", sourceDevice: source, createdAt: now, attachments: attachments, secret: secret)
    }

    // MARK: pager: which pages, which index

    func testTheViewerPagesThroughTheImagesOnlyAndKeepsTheirAttachmentIndex() {
        let images = ViewerImages.of(item([pdf, png, pdf, jpg]))
        XCTAssertEqual(images.map(\.attachmentIndex), [1, 3])
        XCTAssertEqual(images.map(\.attachment.name), ["a.png", "b.jpg"])
        XCTAssertTrue(ViewerImages.of(item([pdf])).isEmpty)
        XCTAssertTrue(ViewerImages.of(item([])).isEmpty)
    }

    func testTheStartPageIsTheTappedImageAndFallsBackToTheFirst() {
        let images = ViewerImages.of(item([pdf, png, pdf, jpg]))
        XCTAssertEqual(ViewerImages.pageFor(images, attachmentIndex: 1), 0)
        XCTAssertEqual(ViewerImages.pageFor(images, attachmentIndex: 3), 1)
        // A file, an unknown index or a negative one: first page, never a crash.
        XCTAssertEqual(ViewerImages.pageFor(images, attachmentIndex: 0), 0)
        XCTAssertEqual(ViewerImages.pageFor(images, attachmentIndex: 99), 0)
        XCTAssertEqual(ViewerImages.pageFor(images, attachmentIndex: -1), 0)
        XCTAssertEqual(ViewerImages.pageFor([], attachmentIndex: 2), 0)
    }

    func testAPageMapsBackToTheAttachmentTheCoreIsAskedFor() {
        let images = ViewerImages.of(item([pdf, png, pdf, jpg]))
        XCTAssertEqual(ViewerImages.attachmentIndexAt(images, page: 0), 1)
        XCTAssertEqual(ViewerImages.attachmentIndexAt(images, page: 1), 3)
        // Out-of-range pages are clamped (the pager may report a page just before the list shrinks).
        XCTAssertEqual(ViewerImages.attachmentIndexAt(images, page: 7), 3)
        XCTAssertEqual(ViewerImages.attachmentIndexAt(images, page: -3), 1)
        XCTAssertNil(ViewerImages.attachmentIndexAt([], page: 0))
    }

    func testThePositionIsShownAsPageOverTotal() {
        XCTAssertEqual(ViewerImages.positionLabel(page: 0, total: 5), "1/5")
        XCTAssertEqual(ViewerImages.positionLabel(page: 1, total: 5), "2/5")
        XCTAssertEqual(ViewerImages.positionLabel(page: 4, total: 5), "5/5")
        XCTAssertEqual(ViewerImages.positionLabel(page: 9, total: 5), "5/5")
        XCTAssertEqual(ViewerImages.positionLabel(page: 0, total: 1), "1/1")
    }

    // MARK: what may be on screen

    func testAHiddenItemShowsNothingUntilTheGatePassed() {
        let secret = item([png, jpg], secret: true)
        XCTAssertEqual(ViewerGate.content(item: secret, loaded: true, unlocked: false), .locked)
        guard case let .pages(images) = ViewerGate.content(item: secret, loaded: true, unlocked: true) else {
            return XCTFail("expected pages")
        }
        XCTAssertEqual(images.count, 2)
    }

    func testANormalItemNeedsNoUnlock() {
        guard case let .pages(images) = ViewerGate.content(item: item([png]), loaded: true, unlocked: false) else {
            return XCTFail("expected pages")
        }
        XCTAssertEqual(images.count, 1)
    }

    func testMissingEmptyAndUnreadableItemsHaveNothingToShow() {
        XCTAssertEqual(ViewerGate.content(item: nil, loaded: false, unlocked: false), .loading)
        XCTAssertEqual(ViewerGate.content(item: nil, loaded: true, unlocked: true), .empty)
        XCTAssertEqual(ViewerGate.content(item: item([pdf]), loaded: true, unlocked: true), .empty)
        let unreadable = TomiItem(id: "u", content: .unreadable(.updateRequired), sourceDevice: source, createdAt: now)
        XCTAssertEqual(ViewerGate.content(item: unreadable, loaded: true, unlocked: true), .empty)
    }

    // MARK: EXIF

    func testExifOrientationsMapToRotationAndMirror() {
        func t(_ o: Int) -> [Int] {
            let transform = ExifOrientation.transformFor(o)
            return [transform.rotationDegrees, transform.flipHorizontal ? 1 : 0]
        }
        XCTAssertEqual(t(1), [0, 0])
        XCTAssertEqual(t(2), [0, 1])
        XCTAssertEqual(t(3), [180, 0])
        XCTAssertEqual(t(4), [180, 1])
        XCTAssertEqual(t(5), [90, 1])
        XCTAssertEqual(t(6), [90, 0])
        XCTAssertEqual(t(7), [-90, 1])
        XCTAssertEqual(t(8), [-90, 0])
        // Missing or invalid value: untouched.
        XCTAssertTrue(ExifOrientation.transformFor(0).isIdentity)
        XCTAssertTrue(ExifOrientation.transformFor(42).isIdentity)
        XCTAssertTrue(ExifOrientation.transformFor(1).isIdentity)
    }

    func testAPortraitPhotoStoredSidewaysIsDisplayedPortrait() {
        // The demo image: stored 1200 x 800 with orientation 6 (rotate 90 clockwise) -> shown 800 x 1200.
        let rotate90 = ExifOrientation.transformFor(6)
        XCTAssertTrue(rotate90.swapsAxes)
        let size = ExifOrientation.displayedSize(width: 1200, height: 800, transform: rotate90)
        XCTAssertEqual([size.width, size.height], [800, 1200])
        let size8 = ExifOrientation.displayedSize(width: 1200, height: 800, transform: ExifOrientation.transformFor(8))
        XCTAssertEqual([size8.width, size8.height], [800, 1200])
        // Orientations that do not turn a quarter keep the size.
        let size3 = ExifOrientation.displayedSize(width: 1200, height: 800, transform: ExifOrientation.transformFor(3))
        XCTAssertEqual([size3.width, size3.height], [1200, 800])
        let size2 = ExifOrientation.displayedSize(width: 1200, height: 800, transform: ExifOrientation.transformFor(2))
        XCTAssertEqual([size2.width, size2.height], [1200, 800])
    }

    func testLargePhotosAreDownsampledByPowersOfTwo() {
        XCTAssertEqual(ImageSampling.sampleSize(width: 1200, height: 800, maxEdge: 4096, maxPixels: 12_000_000), 1)
        XCTAssertEqual(ImageSampling.sampleSize(width: 8000, height: 6000, maxEdge: 4096, maxPixels: 12_000_000), 2)
        // 12 MP limit: 4000 x 3000 = 12 MP passes, 4032 x 3024 = 12.2 MP is halved.
        XCTAssertEqual(ImageSampling.sampleSize(width: 4000, height: 3000, maxEdge: 4096, maxPixels: 12_000_000), 1)
        XCTAssertEqual(ImageSampling.sampleSize(width: 4032, height: 3024, maxEdge: 4096, maxPixels: 12_000_000), 2)
        XCTAssertEqual(ImageSampling.sampleSize(width: 30000, height: 20000, maxEdge: 4096, maxPixels: 12_000_000), 8)
        XCTAssertEqual(ImageSampling.sampleSize(width: 0, height: 0, maxEdge: 4096, maxPixels: 12_000_000), 1)
    }

    // MARK: zoom

    // A 1000 x 500 image fitted in a 1000 x 1000 view is 1000 x 500 at scale 1.
    private let viewW = 1000.0
    private let viewH = 1000.0
    private let fitW = 1000.0
    private let fitH = 500.0

    private func transform(_ p: Placement, zoom: Double, panX: Double = 0, panY: Double = 0, cx: Double = 0, cy: Double = 0) -> Placement {
        ZoomMath.transform(
            p, zoom: zoom, panX: panX, panY: panY, centroidX: cx, centroidY: cy,
            fitWidth: fitW, fitHeight: fitH, viewWidth: viewW, viewHeight: viewH
        )
    }

    func testTheImageIsFittedInTheViewKeepingItsRatio() {
        let a = ZoomMath.fitSize(contentWidth: 2000, contentHeight: 1000, viewWidth: 1000, viewHeight: 1000)
        XCTAssertEqual([a.width, a.height], [1000, 500])
        let b = ZoomMath.fitSize(contentWidth: 800, contentHeight: 1600, viewWidth: 1000, viewHeight: 1000)
        XCTAssertEqual([b.width, b.height], [500, 1000])
        let c = ZoomMath.fitSize(contentWidth: 0, contentHeight: 0, viewWidth: 1000, viewHeight: 1000)
        XCTAssertEqual([c.width, c.height], [0, 0])
    }

    func testPinchZoomIsLimitedAndNeverLeavesAnEmptyBorder() {
        let zoomed = transform(Placement(), zoom: 2)
        XCTAssertEqual(zoomed.scale, 2, accuracy: 0.001)
        // Wider than the view: can move sideways by (2000 - 1000) / 2; as high as 1000 = the view: no vertical move.
        let dragged = transform(zoomed, zoom: 1, panX: 900, panY: 900)
        XCTAssertEqual(dragged.x, 500, accuracy: 0.001)
        XCTAssertEqual(dragged.y, 0, accuracy: 0.001)
        let tooMuch = transform(zoomed, zoom: 100)
        XCTAssertEqual(tooMuch.scale, ZoomMath.maxScale, accuracy: 0.001)
        let tooLittle = transform(zoomed, zoom: 0.01)
        XCTAssertEqual(tooLittle.scale, 1, accuracy: 0.001)
        XCTAssertEqual(tooLittle.x, 0, accuracy: 0.001)
    }

    func testPinchKeepsThePointUnderTheFingersInPlace() {
        // Fingers 200 px right of the centre: after x2 the image has moved 200 px left so that point stays under them.
        let zoomed = transform(Placement(), zoom: 2, cx: 200, cy: 0)
        XCTAssertEqual(zoomed.x, -200, accuracy: 0.001)
    }

    func testDoubleTapZoomsInAroundThePointThenOutAgain() {
        let zoomedIn = ZoomMath.doubleTap(Placement(), tapX: 200, tapY: 0, fitWidth: fitW, fitHeight: fitH, viewWidth: viewW, viewHeight: viewH)
        XCTAssertEqual(zoomedIn.scale, ZoomMath.doubleTapScale, accuracy: 0.001)
        XCTAssertTrue(ZoomMath.isZoomed(zoomedIn))
        XCTAssertLessThan(zoomedIn.x, 0)
        let zoomedOut = ZoomMath.doubleTap(zoomedIn, tapX: 0, tapY: 0, fitWidth: fitW, fitHeight: fitH, viewWidth: viewW, viewHeight: viewH)
        XCTAssertEqual(zoomedOut, Placement())
        XCTAssertFalse(ZoomMath.isZoomed(zoomedOut))
    }

    // MARK: position counter visibility

    func testTheCounterIsVisibleWithTheBarsOrWhileRecentlyActive() {
        XCTAssertTrue(ViewerCounter.isVisible(barsVisible: true, recentlyActive: true))
        XCTAssertTrue(ViewerCounter.isVisible(barsVisible: true, recentlyActive: false))
        XCTAssertTrue(ViewerCounter.isVisible(barsVisible: false, recentlyActive: true))
        XCTAssertFalse(ViewerCounter.isVisible(barsVisible: false, recentlyActive: false))
    }

    func testTheCounterWaitsAFewSecondsBeforeFadingOut() {
        XCTAssertEqual(ViewerCounter.idleMs, 2500)
    }

    // MARK: gallery file name

    func testTheGalleryNameIsSafeAndMatchesTheMimeType() {
        XCTAssertEqual(GalleryName.of("photo.jpg", mime: "image/jpeg"), "photo.jpg")
        XCTAssertEqual(GalleryName.of("photo.JPEG", mime: "image/jpeg"), "photo.JPEG")
        XCTAssertEqual(GalleryName.of("photo", mime: "image/jpeg"), "photo.jpg")
        XCTAssertEqual(GalleryName.of("photo.jpg", mime: "image/png"), "photo.png")
        XCTAssertEqual(GalleryName.of("", mime: "image/webp"), "tomi-image.webp")
        // No path component, no forbidden character, whatever the sender wrote.
        XCTAssertEqual(GalleryName.of("../../etc/passwd", mime: "image/jpeg"), "passwd.jpg")
        XCTAssertEqual(GalleryName.of("a:b*c.png", mime: "image/png"), "a_b_c.png")
        XCTAssertLessThanOrEqual(GalleryName.of(String(repeating: "x", count: 500) + ".jpg", mime: "image/jpeg").count, 104)
    }

    // MARK: menu: no "save to the gallery" for a hidden item (product decision)

    func testAHiddenItemCannotBeSavedToTheGalleryButANormalOneCan() {
        XCTAssertFalse(ViewerActions.canSaveToGallery(item([png], secret: true)))
        XCTAssertTrue(ViewerActions.canSaveToGallery(item([png], secret: false)))
    }
}
