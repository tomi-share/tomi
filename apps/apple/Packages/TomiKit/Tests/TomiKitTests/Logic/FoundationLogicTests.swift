import XCTest
@testable import TomiKit

/// Ports of the TextSanitizer / LinkExtractor behaviour (covered on Android through
/// SharePayloadParserTest and DeepLinkParserTest) and of the auth window and clipboard cases of
/// ItemLogicTest.kt.
final class FoundationLogicTests: XCTestCase {
    func testSanitizerRemovesControlAndBidiCharacters() {
        XCTAssertEqual(TextSanitizer.clean("  a\u{0007}b\r\nc\u{202E}d\u{2066}e\t  "), "ab\nc" + "de")
        XCTAssertNil(TextSanitizer.clean(" \u{0000} \n"))
        XCTAssertNil(TextSanitizer.clean(nil))
        XCTAssertEqual(TextSanitizer.clean("abcdef", maxLength: 3), "abc")
        XCTAssertEqual(TextSanitizer.clean("a\tb"), "a\tb")
    }

    func testLinkExtractorSplitsTitleAndURL() {
        XCTAssertEqual(LinkExtractor.extract("Une page https://example.org/a"), LinkParts(url: "https://example.org/a", leftover: "Une page"))
        XCTAssertEqual(LinkExtractor.extract("https://example.org/a\nTitre"), LinkParts(url: "https://example.org/a", leftover: "Titre"))
        XCTAssertEqual(LinkExtractor.extract("voir https://example.org."), LinkParts(url: "https://example.org", leftover: "voir"))
        XCTAssertEqual(
            LinkExtractor.extract("https://fr.wikipedia.org/wiki/Tomi_(prénom)"),
            LinkParts(url: "https://fr.wikipedia.org/wiki/Tomi_(prénom)", leftover: nil)
        )
        XCTAssertNil(LinkExtractor.extract("lis https://example.org puis réponds"), "URL in the middle")
        XCTAssertNil(LinkExtractor.extract("https://a.org https://b.org"), "two URLs")
        XCTAssertNil(LinkExtractor.extract("ftp://example.org"))
    }

    func testURLValidation() {
        XCTAssertTrue(LinkExtractor.isSingleURL(" https://example.org/x "))
        XCTAssertFalse(LinkExtractor.isSingleURL("https://example.org x"))
        XCTAssertFalse(LinkExtractor.isValidHTTPURL("javascript:alert(1)"))
        XCTAssertFalse(LinkExtractor.isValidHTTPURL("https://"))
        XCTAssertFalse(LinkExtractor.isValidHTTPURL("https://" + String(repeating: "a", count: 2048)))
        XCTAssertTrue(LinkExtractor.isValidHTTPURL("HTTP://Example.org"))
    }

    func testAuthenticationIsValidForTwoMinutes() {
        let clock = Clock()
        let window = AuthWindow(nowMs: { clock.now })
        XCTAssertFalse(window.isValid)
        window.markAuthenticated()
        clock.now = 119_999
        XCTAssertTrue(window.isValid)
        clock.now = 120_000
        XCTAssertFalse(window.isValid)
        window.markAuthenticated()
        window.invalidate()
        XCTAssertFalse(window.isValid)
    }

    func testClipboardIsClearedOnlyIfItStillHoldsOurItem() {
        XCTAssertTrue(ClipboardClearPolicy.shouldClear(expected: 100, current: 100))
        XCTAssertFalse(ClipboardClearPolicy.shouldClear(expected: 100, current: 250))
        XCTAssertTrue(ClipboardClearPolicy.shouldClear(expected: 0, current: 250))
        XCTAssertTrue(ClipboardClearPolicy.shouldClear(expected: 100, current: nil))
        XCTAssertEqual(ClipboardClearPolicy.clearDelayMs, 30_000)
    }

    func testForegroundCheckOnlyClearsAnOverdueClipboardThatIsProvablyOurs() {
        let action = { (expected: Int64, current: Int64?, now: Int64) in
            ClipboardClearPolicy.onForeground(expected: expected, current: current, nowMs: now, deadlineMs: 30_000)
        }
        XCTAssertEqual(action(100, 100, 29_999), .wait)
        XCTAssertEqual(action(100, 100, 30_000), .clear)
        XCTAssertEqual(action(100, 250, 40_000), .forget)
        XCTAssertEqual(action(0, 250, 40_000), .forget)
        XCTAssertEqual(action(100, nil, 40_000), .retry)
    }
}

/// Mutable test clock.
final class Clock: @unchecked Sendable {
    var now: Int64 = 0
}

/// Findings of the independent review of PR-4: Kotlin counts UTF-16 units and Java's `URI` is strict.
final class ReviewFindingsTests: XCTestCase {
    func testURLValidationIsAsStrictAsJava() {
        for bad in [
            "https://my_host.example.com/", "https://-a.com/", "https://a..b/", "https://exämple.com/",
            "https://example.com/a|b", "https://example.com/%zz", "https://a.com/a\\b", "https://a.com/#x#y",
            "https://a.com/%", "https://1abc/",
        ] {
            XCTAssertFalse(LinkExtractor.isValidHTTPURL(bad), bad)
        }
        for good in [
            "https://example.com/", "http://a-b.example.org:8080/p?q=1#f", "https://192.168.1.10/x",
            "https://[::1]/", "https://xn--exmple-cua.com/", "https://example.com/caf%C3%A9",
        ] {
            XCTAssertTrue(LinkExtractor.isValidHTTPURL(good), good)
        }
    }

    func testSecretDetectionIsNeverWeakerThanKotlin() {
        XCTAssertTrue(SecretDetector.looksLikeSecret("passwo\u{301}rd12"))
        XCTAssertTrue(SecretDetector.looksLikeSecret("Ab1!🔑🔑"))
        XCTAssertTrue(SecretDetector.looksLikeSecret("ab12😀😀😀"))
        XCTAssertTrue(SecretDetector.looksLikeSecret("AKIA" + String(repeating: "🔑", count: 7)))
    }

    func testShortTextAndDeepLinkCapsCountUTF16Units() {
        let family = "👨‍👩‍👧‍👦"
        let notShort = TomiItem.readable(
            id: "i", sourceDevice: DeviceRef(id: "pc", name: "PC"), createdAt: Date(),
            text: RichText(plain: String(repeating: family, count: 20))
        )
        XCTAssertEqual(NotificationTapAction.forItem(notShort), .openCard)
        XCTAssertNil(DeepLinkParser.parse("tomi-share://send?text=" + String(repeating: family, count: 4000)))
        XCTAssertEqual(TextSanitizer.clean(String(repeating: family, count: 3), maxLength: 12)?.count, 1)
    }

    func testPairingLinksAreExact() {
        XCTAssertNotNil(DeepLinkParser.parse("tomi-share://pair#4821-crayon-renard", reader: FakeLinkReader()))
        for bad in ["tomi-share://pair:1#c", "tomi-share://pair/#c", "tomi-share://pair?x=1#c", "tomi-share://u@pair#c"] {
            XCTAssertNil(DeepLinkParser.parse(bad, reader: FakeLinkReader()), bad)
        }
    }

    func testDatesAreGregorianWhateverTheUserCalendar() {
        var hebrew = Calendar(identifier: .hebrew)
        hebrew.timeZone = TimeZone(identifier: "Europe/Paris")!
        let date = Date(timeIntervalSince1970: 1_793_872_800)
        // The month is asserted in French: the language is pinned, whatever the machine.
        TomiLocale.current = Locale(identifier: "fr_FR")
        defer { TomiLocale.current = .autoupdatingCurrent }
        XCTAssertTrue(formatDateTime(date, calendar: hebrew).contains("nov."))
    }

    func testRobustnessOfSmallHelpers() {
        XCTAssertEqual(CodeWrap.textWidthPx(viewportWidthPx: 100, gutterPx: .nan, gapPx: 0, endPaddingPx: 0), 1)
        XCTAssertLessThanOrEqual(GalleryName.of(String(repeating: "é", count: 200), mime: "image/jpeg").utf8.count, 255)
        XCTAssertGreaterThan(MonotonicClock.nowMs(), 0)
    }
}
