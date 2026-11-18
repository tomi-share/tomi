import XCTest
@testable import TomiKit

// The estimates of apps/android/.../ui/render/ProgressiveColumn.kt (`textWeight`, `initialItems`): which parts of a long
// text make the first screen of the detail.
final class ProgressiveRevealTests: XCTestCase {
    func testAWeightIsTheCharactersPlusTheLineBreaks() {
        XCTAssertEqual(0, ProgressiveReveal.textWeight(""))
        XCTAssertEqual(5, ProgressiveReveal.textWeight("abcde"))
        XCTAssertEqual(3 + 2 * ProgressiveReveal.lineWeight + 2, ProgressiveReveal.textWeight("a\nb\nc"))
        // UTF-16 code units, as Android counts: an emoji is two.
        XCTAssertEqual(2, ProgressiveReveal.textWeight("😀"))
    }

    func testTheFirstScreenIsUsuallyOnePart() {
        // A part of 4,000 characters is about two screens of an iPhone (a screen holds about 1,700 of weight).
        let screen = ProgressiveReveal.screenWeight(height: 850)
        XCTAssertEqual(1_700, screen)
        XCTAssertEqual(1, ProgressiveReveal.initialItems(count: 10, weight: { _ in 4_000 }, screenWeight: screen))
    }

    func testAShortFirstPartTakesTheNextOnesWithIt() {
        let weights = [200, 300, 5_000, 4_000, 4_000]
        XCTAssertEqual(3, ProgressiveReveal.initialItems(count: 5, weight: { weights[$0] }, screenWeight: 1_700))
    }

    func testNeverMoreThanFourNorThanThereAre() {
        XCTAssertEqual(4, ProgressiveReveal.initialItems(count: 50, weight: { _ in 10 }, screenWeight: 1_700))
        XCTAssertEqual(2, ProgressiveReveal.initialItems(count: 2, weight: { _ in 10 }, screenWeight: 1_700))
        XCTAssertEqual(0, ProgressiveReveal.initialItems(count: 0, weight: { _ in 10 }, screenWeight: 1_700))
        // An empty part still counts for something: no endless first screen.
        XCTAssertEqual(4, ProgressiveReveal.initialItems(count: 9, weight: { _ in 0 }, screenWeight: 1_700))
    }
}
