import XCTest
@testable import TomiKit

// Port of apps/android/.../ui/code/CodeFileLogicTest.kt, without the colour-run tests (no colouring on iOS v1).
// Offsets are UTF-16 code units; the test texts are ASCII, so they equal the Kotlin ones.
final class CodeFileLogicTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_793_872_800)
    private let source = DeviceRef(id: "pc", name: "PC-Bureau")

    private func item(_ attachments: [Attachment], secret: Bool = false, text: RichText? = nil) -> TomiItem {
        .readable(id: "1", sourceDevice: source, createdAt: now, text: text, attachments: attachments, secret: secret)
    }

    private let png = Attachment(name: "a.png", mime: "image/png", size: 10)
    private let pdf = Attachment(name: "a.pdf", mime: "application/pdf", size: 10)
    private let rs = Attachment(name: "main.rs", mime: "text/x-rust", size: 10)
    private let js = Attachment(name: "app.js", mime: "text/javascript", size: 10)
    private let kt = Attachment(name: "Notes.kt", mime: "text/x-kotlin", size: 10)

    // MARK: extension -> language

    func testTheExtensionsOfTheBriefAreCodeFiles() {
        for name in [
            "a.php", "a.rs", "a.js", "a.ts", "a.css", "a.html", "a.json", "a.py", "a.kt", "a.java", "a.sh", "a.go", "a.rb",
            "a.c", "a.cpp", "a.sql", "a.yaml", "a.toml", "a.md",
        ] {
            XCTAssertTrue(CodeLanguages.isCodeFile(name), name)
        }
    }

    func testTheExtensionsWithAGrammarMapToTheLibraryLanguages() {
        let expected = [
            "main.rs": "rust", "app.js": "javascript", "app.mjs": "javascript", "x.jsx": "javascript", "x.ts": "typescript",
            "x.tsx": "tsx", "s.css": "css", "i.html": "html", "i.htm": "html", "d.json": "json", "t.py": "python",
            "T.java": "java", "run.sh": "bash", "run.bash": "bash", "main.go": "go", "m.c": "c", "m.h": "c",
            "c.yaml": "yaml", "c.yml": "yaml", "README.md": "markdown",
            "index.php": "php", "view.phtml": "php", "X.PHP": "php",
        ]
        for (name, grammar) in expected {
            XCTAssertEqual(CodeLanguages.grammarFor(name), grammar, name)
        }
    }

    func testACodeFileWithoutGrammarStillOpensButIsNotColoured() {
        for name in ["a.rb", "a.cpp", "a.sql", "a.toml", "Notes.kt", "build.gradle.kts", "X.KT"] {
            XCTAssertTrue(CodeLanguages.isCodeFile(name), name)
            XCTAssertNil(CodeLanguages.grammarFor(name), name)
        }
    }

    func testExtensionsAreCaseInsensitiveAndNeedARealExtension() {
        XCTAssertTrue(CodeLanguages.isCodeFile("MAIN.RS"))
        XCTAssertEqual(CodeLanguages.grammarFor("MAIN.RS"), "rust")
        XCTAssertEqual(CodeLanguages.extensionOf("dir/sub.name/main.rs"), "rs")
        XCTAssertNil(CodeLanguages.extensionOf("Makefile"))
        XCTAssertNil(CodeLanguages.extensionOf(".env"))
        XCTAssertNil(CodeLanguages.extensionOf("trailing."))
        XCTAssertFalse(CodeLanguages.isCodeFile("notes.pdf"))
        XCTAssertFalse(CodeLanguages.isCodeFile("photo.png"))
        XCTAssertFalse(CodeLanguages.isCodeFile("archive.zip"))
        XCTAssertFalse(CodeLanguages.isCodeFile("rs"))
    }

    func testAnImageIsNeverACodeFile() {
        XCTAssertTrue(CodeFiles.isCodeFile(rs))
        XCTAssertFalse(CodeFiles.isCodeFile(Attachment(name: "diagram.rs", mime: "image/png", size: 10)))
        XCTAssertFalse(CodeFiles.isCodeFile(pdf))
        XCTAssertFalse(CodeFiles.isCodeFile(png))
    }

    // MARK: attachment indices

    func testCodeFilesKeepTheirIndexInTheWholeAttachmentList() {
        let files = CodeFiles.of(item([png, rs, pdf, js, kt]))
        XCTAssertEqual(files.map(\.attachmentIndex), [1, 3, 4])
        XCTAssertEqual(files.map(\.attachment.name), ["main.rs", "app.js", "Notes.kt"])
        XCTAssertEqual(files.map(\.grammar), ["rust", "javascript", nil])
    }

    func testTheStartPositionIsTheFileOfTheTappedAttachment() {
        let files = CodeFiles.of(item([png, rs, pdf, js]))
        XCTAssertEqual(CodeFiles.positionFor(files, attachmentIndex: 1), 0)
        XCTAssertEqual(CodeFiles.positionFor(files, attachmentIndex: 3), 1)
        // Not a code file of the item: the first one.
        XCTAssertEqual(CodeFiles.positionFor(files, attachmentIndex: 0), 0)
        XCTAssertEqual(CodeFiles.positionFor(files, attachmentIndex: 99), 0)
    }

    func testAPositionGivesTheAttachmentIndexTheCoreIsAskedFor() {
        let files = CodeFiles.of(item([png, rs, pdf, js]))
        XCTAssertEqual(CodeFiles.attachmentIndexAt(files, position: 0), 1)
        XCTAssertEqual(CodeFiles.attachmentIndexAt(files, position: 1), 3)
        // Out of range is coerced, an item without code files has none.
        XCTAssertEqual(CodeFiles.attachmentIndexAt(files, position: 7), 3)
        XCTAssertEqual(CodeFiles.attachmentIndexAt(files, position: -2), 1)
        XCTAssertNil(CodeFiles.attachmentIndexAt([], position: 0))
    }

    func testThePositionLabelIsOneBasedAndCoerced() {
        XCTAssertEqual(CodeFiles.positionLabel(position: 0, total: 3), "1/3")
        XCTAssertEqual(CodeFiles.positionLabel(position: 2, total: 3), "3/3")
        XCTAssertEqual(CodeFiles.positionLabel(position: 9, total: 3), "3/3")
        XCTAssertEqual(CodeFiles.positionLabel(position: -1, total: 3), "1/3")
    }

    // MARK: secret gate

    func testAHiddenItemShowsNothingUntilTheGatePassed() {
        let secret = item([rs, js], secret: true)
        XCTAssertEqual(CodeGate.content(item: secret, loaded: true, unlocked: false), .locked)
        guard case let .files(files) = CodeGate.content(item: secret, loaded: true, unlocked: true) else {
            return XCTFail("expected files")
        }
        XCTAssertEqual(files.count, 2)
    }

    func testANormalItemNeedsNoGate() {
        guard case .files = CodeGate.content(item: item([rs]), loaded: true, unlocked: false) else {
            return XCTFail("expected files")
        }
    }

    func testLoadingEmptyAndNoCodeStates() {
        XCTAssertEqual(CodeGate.content(item: nil, loaded: false, unlocked: false), .loading)
        XCTAssertEqual(CodeGate.content(item: nil, loaded: true, unlocked: false), .empty)
        XCTAssertEqual(CodeGate.content(item: item([png, pdf]), loaded: true, unlocked: false), .empty)
        // A hidden item without a code file is locked first: the gate never reveals what the item holds.
        XCTAssertEqual(CodeGate.content(item: item([png], secret: true), loaded: true, unlocked: false), .locked)
    }

    func testOnlyAHiddenItemClosesWhenTheAppLeavesTheScreen() {
        XCTAssertTrue(CodeGate.closesOnStop(item: item([rs], secret: true), changingConfigurations: false))
        // A rotation stops the screen too and must not close it.
        XCTAssertFalse(CodeGate.closesOnStop(item: item([rs], secret: true), changingConfigurations: true))
        XCTAssertFalse(CodeGate.closesOnStop(item: item([rs]), changingConfigurations: false))
        XCTAssertFalse(CodeGate.closesOnStop(item: nil, changingConfigurations: false))
    }

    // MARK: decoding

    func testAFileIsDecodedAsUtf8WithUniformLinesAndExpandedTabs() {
        let result = CodeDecoding.decode(Array("a\r\n\tb\rc\n".utf8))
        XCTAssertEqual(result, .text(original: "a\r\n\tb\rc\n", display: "a\n    b\nc\n"))
    }

    func testAByteOrderMarkIsDropped() {
        let bytes: [UInt8] = [0xEF, 0xBB, 0xBF] + Array("fn main() {}".utf8)
        XCTAssertEqual(CodeDecoding.decode(bytes), .text(original: "fn main() {}", display: "fn main() {}"))
    }

    func testAccentsSurviveAndInvalidBytesDoNotCrash() {
        guard case let .text(original, _) = CodeDecoding.decode(Array("déjà ✓".utf8)) else { return XCTFail("expected text") }
        XCTAssertEqual(original, "déjà ✓")
        guard case let .text(repaired, _) = CodeDecoding.decode([0x66, 0xFF, 0x6F]) else { return XCTFail("expected text") }
        XCTAssertEqual(repaired, "f\u{FFFD}o")
    }

    func testABinaryFileIsRefused() {
        XCTAssertEqual(CodeDecoding.decode([0x50, 0x4B, 0x00, 0x04]), .binary)
    }

    func testAFileOverTheLimitIsRefusedWithoutReadingItAll() {
        let big = [UInt8](repeating: UInt8(ascii: "a"), count: CodeLimits.maxBytes + 1)
        XCTAssertEqual(CodeDecoding.decode(big), .tooLarge)
        XCTAssertEqual(CodeDecoding.read(InputStream(data: Data(big))), .tooLarge)
        let limit = [UInt8](repeating: UInt8(ascii: "a"), count: CodeLimits.maxBytes)
        guard case .text = CodeDecoding.read(InputStream(data: Data(limit))) else { return XCTFail("expected text") }
    }

    func testAnEmptyFileIsAnEmptyDocument() {
        guard case let .text(_, display) = CodeDecoding.read(InputStream(data: Data())) else { return XCTFail("expected text") }
        XCTAssertEqual(display, "")
        XCTAssertEqual(CodeDocument(display).lineCount, 1)
    }

    func testColouringIsLimitedLikeTheLibrary() {
        XCTAssertTrue(CodeLimits.canHighlight(text: "fn main() {}", lineCount: 1))
        XCTAssertFalse(CodeLimits.canHighlight(text: "x", lineCount: CodeLimits.highlightMaxLines + 1))
        XCTAssertTrue(CodeLimits.canHighlight(text: "x", lineCount: CodeLimits.highlightMaxLines))
        XCTAssertFalse(CodeLimits.canHighlight(text: String(repeating: "a", count: CodeLimits.highlightMaxBytes + 1), lineCount: 1))
        // 50 KB of characters that take 2 bytes each is over the byte limit.
        XCTAssertFalse(CodeLimits.canHighlight(text: String(repeating: "é", count: CodeLimits.highlightMaxBytes / 2 + 1), lineCount: 1))
    }

    // MARK: document

    func testADocumentConvertsOffsetsToLinesAndColumns() {
        let doc = CodeDocument("ab\n\ncde\nf")
        XCTAssertEqual(doc.lineCount, 4)
        XCTAssertEqual((0 ..< doc.lineCount).map { doc.line($0) }, ["ab", "", "cde", "f"])
        XCTAssertEqual(doc.lineOf(0), 0)
        XCTAssertEqual(doc.lineOf(2), 0) // the newline belongs to its line
        XCTAssertEqual(doc.lineOf(3), 1)
        XCTAssertEqual(doc.lineOf(4), 2)
        XCTAssertEqual(doc.lineOf(8), 3)
        XCTAssertEqual(doc.lineOf(999), 3)
        XCTAssertEqual(doc.columnOf(6), 2)
        XCTAssertEqual(doc.columnOf(8), 0)
    }

    func testATrailingNewlineAddsAnEmptyLastLine() {
        let doc = CodeDocument("a\n")
        XCTAssertEqual(doc.lineCount, 2)
        XCTAssertEqual(doc.line(1), "")
    }

    func testTheWidestLineIsCappedForTheLayout() {
        XCTAssertEqual(CodeDocument("abc\nabcde\nab").maxLineLength, 5)
        XCTAssertEqual(
            CodeDocument(String(repeating: "a", count: CodeLimits.maxDisplayLineChars * 3)).maxLineLength,
            CodeLimits.maxDisplayLineChars
        )
    }

    // MARK: search

    private let sample = "let Total = 1;\nlet total2 = total + 1;\n// TOTAL\n"

    func testSearchFindsEveryOccurrenceIgnoringCaseInOrder() {
        let matches = CodeSearch.find(text: sample, query: "total")
        XCTAssertEqual(matches.count, 4)
        XCTAssertEqual(matches.map(\.start), [4, 19, 28, 42])
        XCTAssertTrue(matches.allSatisfy { $0.end - $0.start == 5 })
        XCTAssertTrue(zip(matches, matches.dropFirst()).allSatisfy { $0.end <= $1.start })
    }

    func testSearchOccurrencesNeverOverlap() {
        XCTAssertEqual(CodeSearch.find(text: "aaaa", query: "aa").map(\.start), [0, 2])
    }

    func testAnEmptyQueryOrNoMatchFindsNothing() {
        XCTAssertTrue(CodeSearch.find(text: sample, query: "").isEmpty)
        XCTAssertTrue(CodeSearch.find(text: sample, query: "zzz").isEmpty)
        XCTAssertTrue(CodeSearch.find(text: "", query: "a").isEmpty)
    }

    func testSearchStopsAtTheListingCap() {
        let many = String(repeating: "a", count: CodeSearch.maxMatches + 100)
        XCTAssertEqual(CodeSearch.find(text: many, query: "a").count, CodeSearch.maxMatches)
    }

    func testSearchIgnoresCaseOfNonAsciiLetters() {
        XCTAssertEqual(CodeSearch.find(text: "Été été", query: "ÉTÉ").map(\.start), [0, 4])
    }

    func testNextAndPreviousWrapAround() {
        XCTAssertEqual(CodeSearch.next(current: 0, count: 4), 1)
        XCTAssertEqual(CodeSearch.next(current: 3, count: 4), 0)
        XCTAssertEqual(CodeSearch.previous(current: 0, count: 4), 3)
        XCTAssertEqual(CodeSearch.previous(current: 2, count: 4), 1)
        // A single occurrence stays on itself.
        XCTAssertEqual(CodeSearch.next(current: 0, count: 1), 0)
        XCTAssertEqual(CodeSearch.previous(current: 0, count: 1), 0)
        // Nothing to navigate to.
        XCTAssertEqual(CodeSearch.next(current: -1, count: 0), -1)
        XCTAssertEqual(CodeSearch.previous(current: -1, count: 0), -1)
    }

    func testTheCounterIsOneBasedAndEmptyWhenThereIsNoMatch() {
        XCTAssertEqual(CodeSearch.counter(current: 0, count: 4), "1/4")
        XCTAssertEqual(CodeSearch.counter(current: 3, count: 4), "4/4")
        XCTAssertEqual(CodeSearch.counter(current: -1, count: 0), "0/0")
        XCTAssertEqual(CodeSearch.counter(current: 0, count: 0), "0/0")
        XCTAssertEqual(CodeSearch.counter(current: 4, count: CodeSearch.maxMatches), "5/\(CodeSearch.maxMatches)+")
    }

    func testTheFirstOccurrenceToSelectIsTheNextOneFromAnOffsetElseTheFirst() {
        let matches = CodeSearch.find(text: sample, query: "total")
        XCTAssertEqual(CodeSearch.firstFrom(matches, fromOffset: 0), 0)
        XCTAssertEqual(CodeSearch.firstFrom(matches, fromOffset: 5), 1)
        XCTAssertEqual(CodeSearch.firstFrom(matches, fromOffset: 42), 3)
        XCTAssertEqual(CodeSearch.firstFrom(matches, fromOffset: 1000), 0) // past the last one: wrap to the first
        XCTAssertEqual(CodeSearch.firstFrom([], fromOffset: 0), -1)
    }

    func testAnOccurrenceIsLocatedByLineAndColumnToScrollToIt() {
        let doc = CodeDocument(sample)
        let matches = CodeSearch.find(text: doc.text, query: "total")
        XCTAssertEqual(matches.map { doc.lineOf($0.start) }, [0, 1, 1, 2])
        XCTAssertEqual(matches.map { doc.columnOf($0.start) }, [4, 4, 13, 3])
    }

    func testMatchesAreBucketedByLineWithTheirRank() {
        let doc = CodeDocument(sample)
        let byLine = CodeLines.bucketMatches(doc, matches: CodeSearch.find(text: doc.text, query: "total"))
        XCTAssertEqual(Set(byLine.keys), [0, 1, 2])
        XCTAssertEqual(byLine[0]?.map(\.index), [0])
        XCTAssertEqual(byLine[1]?.map(\.index), [1, 2])
        XCTAssertEqual(byLine[2]?.map(\.index), [3])
        XCTAssertEqual(byLine[0]?.map { [$0.startCol, $0.endCol] }, [[4, 9]])
    }

    func testAMatchAcrossTwoLinesIsCutAtTheLineEnd() {
        let doc = CodeDocument("ab\ncd")
        let byLine = CodeLines.bucketMatches(doc, matches: [CodeMatch(start: 1, end: 4)])
        XCTAssertEqual(byLine[0]?.map { [$0.startCol, $0.endCol] }, [[1, 2]])
        XCTAssertEqual(byLine[1]?.map { [$0.startCol, $0.endCol] }, [[0, 1]])
    }

    func testHorizontalScrollOnlyMovesWhenTheColumnIsOutOfView() {
        // 60 characters visible from the character 0.
        XCTAssertNil(CodeLines.horizontalScrollTarget(col: 20, visibleStartChar: 0, visibleChars: 60))
        // Far to the right: scrolls so that the column sits in the first third.
        XCTAssertEqual(CodeLines.horizontalScrollTarget(col: 120, visibleStartChar: 0, visibleChars: 60), 100)
        // Back to the left of the view.
        XCTAssertEqual(CodeLines.horizontalScrollTarget(col: 3, visibleStartChar: 50, visibleChars: 60), 0)
        XCTAssertNil(CodeLines.horizontalScrollTarget(col: 3, visibleStartChar: 0, visibleChars: 0))
    }

    // MARK: line wrap

    func testTheWrapToggleFlipsTheState() {
        XCTAssertTrue(CodeWrap.toggled(false))
        XCTAssertFalse(CodeWrap.toggled(true))
        XCTAssertFalse(CodeWrap.toggled(CodeWrap.toggled(false)))
    }

    func testTheTextWidthIsWhatTheGutterThePaddingsLeave() {
        XCTAssertEqual(CodeWrap.textWidthPx(viewportWidthPx: 400, gutterPx: 40, gapPx: 50, endPaddingPx: 10), 300)
        // Never zero or negative: a measure needs a width.
        XCTAssertEqual(CodeWrap.textWidthPx(viewportWidthPx: 20, gutterPx: 40, gapPx: 50, endPaddingPx: 10), 1)
    }

    func testARowAlreadyComfortablyVisibleIsNotScrolled() {
        XCTAssertNil(CodeWrap.scrollDelta(rowTop: 300, rowBottom: 319, viewportStart: 0, viewportEnd: 1000, margin: 40))
    }

    func testARowBelowTheViewportIsScrolledToAThirdOfIt() {
        // Row at 1500; target top = 1000 / 3 -> content moves up by 1500 - 333.33.
        let delta = CodeWrap.scrollDelta(rowTop: 1500, rowBottom: 1519, viewportStart: 0, viewportEnd: 1000, margin: 40)
        XCTAssertEqual(try XCTUnwrap(delta), 1500 - 1000.0 / 3, accuracy: 0.01)
    }

    func testARowAboveTheViewportScrollsBackwards() {
        let delta = CodeWrap.scrollDelta(rowTop: -200, rowBottom: -181, viewportStart: 0, viewportEnd: 1000, margin: 40)
        XCTAssertLessThan(try XCTUnwrap(delta), 0)
    }

    func testARowAtTheEdgeIsMovedIn() {
        // Inside the viewport, but within the margin of its bottom.
        XCTAssertNotNil(CodeWrap.scrollDelta(rowTop: 970, rowBottom: 989, viewportStart: 0, viewportEnd: 1000, margin: 40))
    }

    func testAnEmptyViewportScrollsNothing() {
        XCTAssertNil(CodeWrap.scrollDelta(rowTop: 10, rowBottom: 29, viewportStart: 0, viewportEnd: 0, margin: 40))
    }
}
