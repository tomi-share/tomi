import SwiftUI
import XCTest
@testable import TomiDesign

final class TomiDesignTests: XCTestCase {
    private typealias L = TomiTokens.ColorLight
    private typealias D = TomiTokens.ColorDark

    /// Text pairs of the charter (§4.3) keep WCAG AA (4.5:1) in both appearances.
    func testTextPairsMeetAAInBothAppearances() {
        let light: [(String, UInt32, UInt32)] = [
            ("text/bg", L.text, L.bg), ("text/surface", L.text, L.surface),
            ("text-muted/bg", L.textMuted, L.bg), ("text-muted/surface", L.textMuted, L.surface),
            ("brand/bg", L.brand, L.bg), ("brand/surface", L.brand, L.surface),
            ("on-brand/brand", L.onBrand, L.brand),
            ("secondary/bg", L.secondary, L.bg), ("secondary/surface", L.secondary, L.surface),
            ("secret/surface", L.secret, L.surface), ("secret/secret-container", L.secret, L.secretContainer),
            ("success/surface", L.success, L.surface), ("error/surface", L.error, L.surface),
        ]
        let dark: [(String, UInt32, UInt32)] = [
            ("text/bg", D.text, D.bg), ("text/surface", D.text, D.surface),
            ("text-muted/bg", D.textMuted, D.bg), ("text-muted/surface", D.textMuted, D.surface),
            ("brand/bg", D.brand, D.bg), ("brand/surface", D.brand, D.surface),
            ("on-brand/brand", D.onBrand, D.brand),
            ("secondary/bg", D.secondary, D.bg), ("secondary/surface", D.secondary, D.surface),
            ("secret/surface", D.secret, D.surface), ("secret/secret-container", D.secret, D.secretContainer),
            ("success/surface", D.success, D.surface), ("error/surface", D.error, D.surface),
        ]
        for (name, fg, bg) in light + dark {
            XCTAssertGreaterThanOrEqual(tomiContrastRatio(fg, bg), 4.5, name)
        }
    }

    /// Interactive outlines keep 3:1 (WCAG 1.4.11).
    func testControlOutlinesMeetThreeToOne() {
        XCTAssertGreaterThanOrEqual(tomiContrastRatio(L.borderStrong, L.bg), 3)
        XCTAssertGreaterThanOrEqual(tomiContrastRatio(L.borderStrong, L.surface), 3)
        XCTAssertGreaterThanOrEqual(tomiContrastRatio(D.borderStrong, D.bg), 3)
        XCTAssertGreaterThanOrEqual(tomiContrastRatio(D.borderStrong, D.surface), 3)
    }

    /// The sun yellow is reserved for the symbol on night: it is no interface colour.
    func testTheSunIsNoInterfaceColour() {
        let interface: [UInt32] = [
            L.bg, L.surface, L.surfaceAlt, L.text, L.textMuted, L.border, L.borderStrong, L.brand, L.onBrand,
            L.brandContainer, L.secondary, L.secret, L.secretContainer, L.success, L.error,
            D.bg, D.surface, D.surfaceAlt, D.text, D.textMuted, D.border, D.borderStrong, D.brand, D.onBrand,
            D.brandContainer, D.secondary, D.secret, D.secretContainer, D.success, D.error,
        ]
        XCTAssertFalse(interface.contains(TomiTokens.ColorIdentity.sun))
    }

    func testContrastRatioOfKnownPairs() {
        XCTAssertEqual(tomiContrastRatio(0x000000, 0xFFFFFF), 21, accuracy: 0.001)
        XCTAssertEqual(tomiContrastRatio(0x777777, 0x777777), 1, accuracy: 0.001)
    }

    /// Charter §6.3: no circular-arrow icon, no padlock on hidden items.
    func testNoCircularArrowAndNoPadlock() {
        for icon in TomiIcon.allCases {
            let name = icon.systemName
            XCTAssertFalse(name.contains("arrow.clockwise") || name.contains("arrow.counterclockwise"), name)
            XCTAssertFalse(name.contains("circlepath"), name)
            XCTAssertFalse(name.split(separator: ".").contains("lock"), name)
        }
        XCTAssertEqual(Set(TomiIcon.allCases.map(\.systemName)).count, TomiIcon.allCases.count, "one symbol per meaning")
    }

    /// The symbol keeps the reference geometry: box x 22-230, y 34-222 in the 256 grid.
    func testSymbolGeometry() {
        let box = TomiSymbol().path(in: CGRect(x: 0, y: 0, width: 256, height: 256)).boundingRect
        XCTAssertEqual(box.minX, 22, accuracy: 0.5)
        XCTAssertEqual(box.maxX, 230, accuracy: 0.5)
        XCTAssertEqual(box.minY, 34, accuracy: 0.5)
        XCTAssertEqual(box.maxY, 222, accuracy: 0.5)
        let cuts = TomiSymbol.ringOutline()
        XCTAssertEqual(cuts.first!.x, 61.93, accuracy: 0.01, "outer cut at 218°, as logo.svg")
        XCTAssertEqual(cuts.first!.y, 70.13, accuracy: 0.01)
    }

    func testBundledFontsRegister() {
        XCTAssertTrue(TomiFonts.areAvailable)
    }

    func testTextStylesFollowTheCharter() {
        XCTAssertEqual(TomiTextStyle.body.size, 15)
        XCTAssertEqual(TomiTextStyle.body.lineHeight, 22)
        XCTAssertEqual(TomiTextStyle.caption.size, 12, "minimum size")
        XCTAssertTrue(TomiTextStyle.pairingCode.isMono)
        XCTAssertEqual(TomiRadius.medium, 8)
        XCTAssertEqual(TomiRadius.large, 12)
        XCTAssertEqual(TomiMotion.undoWindow, 3)
        XCTAssertEqual(TomiMotion.secretClipboardClear, 30)
    }
}
