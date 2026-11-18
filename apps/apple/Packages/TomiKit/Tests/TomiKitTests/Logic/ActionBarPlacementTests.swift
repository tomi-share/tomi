import TomiKit
import XCTest

final class ActionBarPlacementTests: XCTestCase {
    func testClassicIPhonesKeepTheBarAtTheBottom() {
        // Portrait: no side inset. Landscape with a notch: the same inset on both sides.
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 0, trailing: 0), .bottom)
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 59, trailing: 59), .bottom)
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 47, trailing: 47), .bottom)
    }

    func testTheSystemVerticalBarDecidesTheSide() {
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: .trailing, leading: 0, trailing: 100), .side(.trailing, width: 100))
        // An app on the left of a split view: the controls follow to the left edge.
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: .leading, leading: 84, trailing: 0), .side(.leading, width: 84))
    }

    func testTheUnfoldedDisplayInLandscapeUsesItsStrip() {
        // Inner display of iPhone Duo in landscape: status bar in an 84 pt strip on the right, like folded.
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: .trailing, leading: 0, trailing: 84), .side(.trailing, width: 84))
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 0, trailing: 84), .side(.trailing, width: 84))
    }

    func testTheUnfoldedDisplayInPortraitKeepsTheBarAtTheBottom() {
        // Status bar at the top, no strip on the side.
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 0, trailing: 0), .bottom)
    }

    func testAStripTooNarrowForTheBarKeepsItAtTheBottom() {
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: .trailing, leading: 0, trailing: 40), .bottom)
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 0, trailing: 63), .bottom)
    }

    func testWithoutTheSystemEdgeOnlyAOneSidedStripCounts() {
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 84, trailing: 0), .side(.leading, width: 84))
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: 20, trailing: 84), .bottom)
    }

    func testTheVerticalBarStaysAboveTheCamera() {
        // Folded in landscape: the camera occupies 84 × 82 pt at the bottom of the strip.
        XCTAssertEqual(ActionBarPlacement.sideBottomClearance(safeBottom: 34, occupiedCorner: 82), 82)
        // Folded in portrait: nothing at the bottom, the home indicator only.
        XCTAssertEqual(ActionBarPlacement.sideBottomClearance(safeBottom: 34, occupiedCorner: 0), 34)
        XCTAssertEqual(ActionBarPlacement.sideBottomClearance(safeBottom: .nan, occupiedCorner: -5), 0)
    }

    func testInvalidInsetsNeverMoveTheBar() {
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: nil, leading: .nan, trailing: .infinity), .bottom)
        XCTAssertEqual(ActionBarPlacement.resolve(systemEdge: .trailing, leading: 0, trailing: -100), .bottom)
        XCTAssertFalse(ActionBarPlacement.bottom.isSide)
        XCTAssertTrue(ActionBarPlacement.side(.trailing, width: 90).isSide)
    }
}

final class TopCornerBandTests: XCTestCase {
    private func band(
        width: Double = 669, top: Double, leading: Double = 0, trailing: Double = 0,
        topLeading: (Double, Double) = (0, 0), topTrailing: (Double, Double) = (0, 0)
    ) -> TopCornerBand? {
        TopCornerBand.resolve(
            width: width, safeTop: top, safeLeading: leading, safeTrailing: trailing,
            topLeadingCorner: (width: topLeading.0, height: topLeading.1),
            topTrailingCorner: (width: topTrailing.0, height: topTrailing.1)
        )
    }

    func testTheUnfoldedDuoInPortraitFreesTheBandBesideTheStatusBlock() {
        // Measured on the simulator: top inset 82, time, camera and Wi-Fi in 144 × 82 at the top right.
        XCTAssertEqual(band(top: 82, topTrailing: (144, 82)), TopCornerBand(height: 82, leading: 0, trailing: 144))
    }

    func testAClassicIPhoneHasNoBand() {
        // Dynamic Island in the middle: no occupied corner, or rounded corners shorter than the inset.
        XCTAssertNil(band(width: 402, top: 62))
        XCTAssertNil(band(width: 402, top: 62, topLeading: (55, 55), topTrailing: (55, 55)))
    }

    func testTheSideStripOfIPhoneDuoHasNoBand() {
        // Unfolded in landscape: the block is in the strip on the right, nothing at the top.
        XCTAssertNil(band(width: 951, top: 0, trailing: 84, topTrailing: (84, 124)))
        // Folded: a small top inset beside the strip.
        XCTAssertNil(band(width: 466, top: 24, trailing: 84, topTrailing: (84, 124)))
    }

    func testTheWindowControlsOfAnIPadWindowMakeABand() {
        // Measured on the iPad Air 11 simulator (iPadOS 27.2), window resized to 400 × 655: top inset 10, controls in
        // 66 × 53 at the top left, rounded corners of 22 × 22 elsewhere. The header goes beside the controls.
        let window = band(width: 400, top: 10, topLeading: (66, 53), topTrailing: (22, 22))
        XCTAssertEqual(window, TopCornerBand(height: 53, leading: 66, trailing: 0, inset: 10))
        // On the line of the controls, measured centred at 31 pt; no glass of ours behind them.
        XCTAssertEqual(window?.rowCenter, 31.5)
        XCTAssertEqual(window?.holdsStatus, false)
        // The same window moved to the top of the screen, under the status bar: controls in 66 × 75 under 32 pt,
        // measured centred at 53 pt.
        let atTop = band(width: 400, top: 32, topLeading: (66, 75))
        XCTAssertEqual(atTop, TopCornerBand(height: 75, leading: 66, trailing: 0, inset: 32))
        XCTAssertEqual(atTop?.rowCenter, 53.5)
        // Full screen: no occupied corner, the status bar of the iPad above the page.
        XCTAssertNil(band(width: 820, top: 32))
        // Only rounded window corners: no band.
        XCTAssertNil(band(width: 400, top: 10, topLeading: (22, 22), topTrailing: (22, 22)))
    }

    func testABandTooNarrowForTheHeaderIsNotUsed() {
        XCTAssertNil(band(width: 400, top: 82, topLeading: (60, 82), topTrailing: (144, 82)))
    }

    func testTheTopRowLinesUpWithTheStatusContent() {
        let band = TopCornerBand(height: 82, leading: 0, trailing: 144)
        // Measured: time and camera centred at 48 pt in the 82 pt band.
        XCTAssertEqual(band.rowCenter, 47.83, accuracy: 0.01)
        XCTAssertTrue(band.holdsStatus)
        XCTAssertEqual(band.backdropHeight, 55)
        XCTAssertEqual(TopCornerBand.headerCenter(band: band, safeTop: 82), band.rowCenter)
        // Folded, or unfolded in landscape: no top inset, the row centres where the camera is.
        XCTAssertEqual(TopCornerBand.headerCenter(band: nil, safeTop: 0), 48)
        XCTAssertEqual(TopCornerBand.headerCenter(band: nil, safeTop: 20), 28)
        // Classic iPhone: under the status bar, as usual.
        XCTAssertNil(TopCornerBand.headerCenter(band: nil, safeTop: 62))
    }

    func testInvalidValuesGiveNoBand() {
        XCTAssertNil(band(top: .nan, topTrailing: (144, 82)))
        XCTAssertNil(band(top: 82, topTrailing: (.infinity, 82)))
    }
}
