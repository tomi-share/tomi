import XCTest
@testable import TomiKit

// Port of apps/android/.../ui/feed/AttachmentLogicTest.kt (same cases, same values; lengths in UTF-16 units like Kotlin).
final class AttachmentLogicTests: XCTestCase {
    private func image(_ name: String = "a.jpg", thumbnail: Data? = Data([1, 2, 3])) -> Attachment {
        Attachment(name: name, mime: "image/jpeg", size: 1000, thumbnail: thumbnail)
    }

    private func file(_ name: String) -> Attachment {
        Attachment(name: name, mime: "application/pdf", size: 1000)
    }

    // MARK: extension

    func testSplitsTheLastExtension() {
        let parts = AttachmentLogic.splitExtension("Courrier banque.pdf")
        XCTAssertEqual(parts.radical, "Courrier banque")
        XCTAssertEqual(parts.extension, ".pdf")
        XCTAssertEqual(AttachmentLogic.splitExtension("archive.tar.gz").extension, ".gz")
    }

    func testNoExtensionWhenThereIsNone() {
        for name in ["LISEZMOI", ".gitignore", "fin.", "note. finale", "a." + String(repeating: "x", count: 11)] {
            let parts = AttachmentLogic.splitExtension(name)
            XCTAssertEqual(parts.radical, name)
            XCTAssertEqual(parts.extension, "")
        }
    }

    func testBadgeShowsTheExtensionInCapitalsUpToFourLetters() {
        XCTAssertEqual(AttachmentLogic.badgeExtension("devis.pdf"), "PDF")
        XCTAssertEqual(AttachmentLogic.badgeExtension("FeedViewModel.kt"), "KT")
        XCTAssertEqual(AttachmentLogic.badgeExtension("photo.webp"), "WEBP")
        XCTAssertEqual(AttachmentLogic.badgeExtension("a.tar.gz"), "GZ")
    }

    func testBadgeFallsBackToAnIconWithoutUsableExtension() {
        XCTAssertNil(AttachmentLogic.badgeExtension("schema.drawio"))
        XCTAssertNil(AttachmentLogic.badgeExtension("LISEZMOI"))
        XCTAssertNil(AttachmentLogic.badgeExtension(".gitignore"))
        XCTAssertNil(AttachmentLogic.badgeExtension("a.p-f"))
    }

    // MARK: cutting before the extension

    /// A text "fits" when it has at most `width` UTF-16 units (one line of that width).
    private func fit(_ name: String, _ width: Int) -> String {
        AttachmentLogic.fitFileName(name) { $0.utf16.count <= width }
    }

    func testANameThatFitsIsUntouched() {
        XCTAssertEqual(fit("devis.pdf", 20), "devis.pdf")
        XCTAssertEqual(fit("devis.pdf", 9), "devis.pdf")
    }

    func testTheExtensionIsKeptAndTheRadicalIsCutAsLateAsPossible() {
        let name = "Courrier banque fonds de garantie locative résiliation du bail 2026.pdf"
        let cut = fit(name, 31)
        XCTAssertEqual(cut, "Courrier banque fonds de g….pdf")
        XCTAssertEqual(cut.utf16.count, 31)
        XCTAssertTrue(name.hasPrefix(cut.replacingOccurrences(of: "….pdf", with: "")))
    }

    func testTheCutRadicalLosesItsTrailingSpace() {
        // "Courrier banque" + "…" would otherwise read "Courrier banque …".
        XCTAssertEqual(fit("Courrier banque fonds.pdf", 21), "Courrier banque….pdf")
    }

    func testANameWithoutExtensionIsCutAtTheEnd() {
        XCTAssertEqual(fit("abcdefghijklmnopqrstuvwxyz", 10), "abcdefghi…")
    }

    func testWhenNothingFitsTheFloorIsTheEllipsisAndTheExtension() {
        XCTAssertEqual(fit("a-very-long-name.pdf", 2), "….pdf")
    }

    func testNeverCutsInTheMiddleOfASurrogatePair() {
        // 3 characters of radical would end in the middle of the emoji (2 UTF-16 units): 2 are kept instead.
        let cut = AttachmentLogic.fitFileName("ab😀cdef.png") { $0.utf16.count <= 8 }
        XCTAssertEqual(cut, "ab….png")
        XCTAssertFalse(cut.utf16.contains { UTF16.isLeadSurrogate($0) || UTF16.isTrailSurrogate($0) })
    }

    func testTheSearchIsLogarithmic() {
        var calls = 0
        _ = AttachmentLogic.fitFileName(String(repeating: "x", count: 4000) + ".pdf") {
            calls += 1
            return $0.utf16.count <= 2000
        }
        XCTAssertLessThanOrEqual(calls, 20, "\(calls) measures")
    }

    // MARK: plan

    private func items(_ plan: AttachmentLogic.Plan, file: StaticString = #filePath, line: UInt = #line) -> (entries: [AttachmentLogic.Entry], more: Int) {
        guard case let .items(entries, more) = plan else {
            XCTFail("expected items, got \(plan)", file: file, line: line)
            return ([], -1)
        }
        return (entries, more)
    }

    private func mosaic(_ plan: AttachmentLogic.Plan, file: StaticString = #filePath, line: UInt = #line) -> (shown: Int, more: Int) {
        guard case let .mosaic(shown, more) = plan else {
            XCTFail("expected a mosaic, got \(plan)", file: file, line: line)
            return (-1, -1)
        }
        return (shown, more)
    }

    func testASingleImageIsAFrameInTheFeedAndTheDetail() {
        for expanded in [false, true] {
            let plan = items(AttachmentLogic.plan([image()], expanded: expanded, showThumbnails: true))
            XCTAssertEqual(plan.entries.map(\.look), [.frame])
            XCTAssertEqual(plan.more, 0)
        }
    }

    func testASingleImageWithoutThumbnailIsATile() {
        let plan = items(AttachmentLogic.plan([image(thumbnail: nil)], expanded: false, showThumbnails: true))
        XCTAssertEqual(plan.entries.map(\.look), [.tile])
    }

    func testImagesOnlyMakeAMosaicOfThreeCellsAndCountTheRest() {
        let plan = mosaic(AttachmentLogic.plan(Array(repeating: image(), count: 5), expanded: false, showThumbnails: true))
        XCTAssertEqual(plan.shown, 3)
        XCTAssertEqual(plan.more, 2)
    }

    func testTwoOrThreeImagesHaveNoPlusCell() {
        let two = mosaic(AttachmentLogic.plan(Array(repeating: image(), count: 2), expanded: false, showThumbnails: true))
        XCTAssertEqual(two.shown, 2)
        XCTAssertEqual(two.more, 0)
        let three = mosaic(AttachmentLogic.plan(Array(repeating: image(), count: 3), expanded: false, showThumbnails: true))
        XCTAssertEqual(three.shown, 3)
        XCTAssertEqual(three.more, 0)
    }

    func testFourImagesGiveAPlusOneCell() {
        let plan = mosaic(AttachmentLogic.plan(Array(repeating: image(), count: 4), expanded: false, showThumbnails: true))
        XCTAssertEqual(plan.shown, 3)
        XCTAssertEqual(plan.more, 1)
    }

    func testAFourthImageWithoutThumbnailKeepsTheMosaic() {
        let batch = Array(repeating: image(), count: 3) + [image(thumbnail: nil)]
        let plan = mosaic(AttachmentLogic.plan(batch, expanded: false, showThumbnails: true))
        XCTAssertEqual(plan.more, 1)
    }

    func testNoAttachmentIsNoItem() {
        let plan = items(AttachmentLogic.plan([], expanded: false, showThumbnails: true))
        XCTAssertTrue(plan.entries.isEmpty)
        XCTAssertEqual(plan.more, 0)
    }

    func testAnEmptyNameIsUntouchedAndADotFileKeepsItsLastExtension() {
        XCTAssertEqual(fit("", 5), "")
        XCTAssertEqual(AttachmentLogic.splitExtension(".env.local").extension, ".local")
        XCTAssertEqual(AttachmentLogic.splitExtension(".env.local").radical, ".env")
    }

    func testAMixedBatchIsThreeTilesThenTheRest() {
        let batch = [image(), file("a.pdf"), file("b.txt"), file("c.txt"), file("d.txt")]
        let plan = items(AttachmentLogic.plan(batch, expanded: false, showThumbnails: true))
        // In a list, the image is a tile with a mini-thumbnail (the frame is for a lone image).
        XCTAssertEqual(plan.entries.map(\.look), [.tile, .tile, .tile])
        XCTAssertEqual(plan.entries.map(\.index), [0, 1, 2])
        XCTAssertEqual(plan.more, 2)
    }

    func testImagesWithAMissingThumbnailFallBackToTiles() {
        let plan = AttachmentLogic.plan([image(), image(thumbnail: nil)], expanded: false, showThumbnails: true)
        XCTAssertEqual(items(plan).entries.count, 2)
    }

    func testAHiddenItemNeverShowsThumbnailsNorAMosaic() {
        let plan = items(AttachmentLogic.plan(Array(repeating: image(), count: 4), expanded: false, showThumbnails: false))
        XCTAssertTrue(plan.entries.allSatisfy { $0.look == .tile })
        XCTAssertEqual(plan.more, 1)
        let single = items(AttachmentLogic.plan([image()], expanded: true, showThumbnails: false))
        XCTAssertEqual(single.entries.map(\.look), [.tile])
    }

    func testTheDetailListsEverythingInOrder() {
        let all = [image("1.jpg"), file("2.pdf"), image("3.jpg"), file("4.txt"), file("5.txt")]
        let plan = items(AttachmentLogic.plan(all, expanded: true, showThumbnails: true))
        XCTAssertEqual(plan.entries.map(\.index), [0, 1, 2, 3, 4])
        XCTAssertEqual(plan.more, 0)
        XCTAssertEqual(plan.entries.map(\.look), [.frame, .tile, .frame, .tile, .tile])
    }

    func testTheDetailOfImagesOnlyIsNotAMosaic() {
        let plan = AttachmentLogic.plan(Array(repeating: image(), count: 5), expanded: true, showThumbnails: true)
        XCTAssertEqual(items(plan).entries.count, 5)
    }

    // MARK: summary

    func testSummaryFromTwoAttachmentsOn() {
        XCTAssertFalse(AttachmentLogic.showsSummary([file("a.pdf")]))
        XCTAssertTrue(AttachmentLogic.showsSummary([file("a.pdf"), file("b.pdf")]))
        XCTAssertEqual(AttachmentLogic.totalSize([file("a.pdf"), file("b.pdf")]), 2000)
        XCTAssertTrue(AttachmentLogic.isAllImages([image(), image()]))
        XCTAssertFalse(AttachmentLogic.isAllImages([image(), file("a.pdf")]))
        XCTAssertFalse(AttachmentLogic.isAllImages([]))
    }
}
