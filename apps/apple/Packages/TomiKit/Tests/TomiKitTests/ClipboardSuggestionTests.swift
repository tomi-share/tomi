import XCTest
@testable import TomiKit

/// What the send screen offers from the clipboard, and for how long.
final class ClipboardSuggestionTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    // MARK: Less than a minute

    func testACopySeenByTheAppIsOfferedForOneMinute() {
        var clock = ClipboardClock()
        clock.observe(count: 4, now: t0)
        XCTAssertFalse(clock.isFresh(now: t0), "what was there before the app looked has no known age")
        clock.observe(count: 5, now: t0.addingTimeInterval(10))
        XCTAssertTrue(clock.isFresh(now: t0.addingTimeInterval(10)))
        XCTAssertTrue(clock.isFresh(now: t0.addingTimeInterval(69)))
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(70)), "a minute later it is no longer offered")
        // Looking again without a change does not make it young again.
        clock.observe(count: 5, now: t0.addingTimeInterval(65))
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(71)))
    }

    func testOnIOSWhatWasCopiedBeforeOpeningTomiCounts() {
        var clock = ClipboardClock()
        clock.observe(count: 9, now: t0, firstLookCounts: true)
        XCTAssertTrue(clock.isFresh(now: t0.addingTimeInterval(30)))
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(61)))
        // Back in front later with the same clipboard: nothing new.
        clock.observe(count: 9, now: t0.addingTimeInterval(600), firstLookCounts: true)
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(601)))
        clock.observe(count: 10, now: t0.addingTimeInterval(700), firstLookCounts: true)
        XCTAssertTrue(clock.isFresh(now: t0.addingTimeInterval(701)))
    }

    func testACopyMadeInTomiIsNotOfferedBack() {
        var clock = ClipboardClock()
        clock.observe(count: 1, now: t0)
        clock.observe(count: 2, now: t0.addingTimeInterval(1))
        clock.ignore(count: 3)
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(2)))
        clock.observe(count: 3, now: t0.addingTimeInterval(3))
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(3)), "the same count: still Tomi's own copy")
        clock.observe(count: 4, now: t0.addingTimeInterval(5))
        XCTAssertTrue(clock.isFresh(now: t0.addingTimeInterval(5)), "copied elsewhere since")
    }

    func testASuggestionTakenOrDismissedDoesNotComeBack() {
        var clock = ClipboardClock()
        clock.observe(count: 1, now: t0)
        clock.observe(count: 2, now: t0.addingTimeInterval(1))
        clock.consume()
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(2)))
        clock.observe(count: 2, now: t0.addingTimeInterval(3))
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(3)))
    }

    func testAClockThatWentBackOffersNothing() {
        var clock = ClipboardClock()
        clock.observe(count: 1, now: t0)
        clock.observe(count: 2, now: t0.addingTimeInterval(100))
        XCTAssertFalse(clock.isFresh(now: t0), "the time of the device was changed")
    }

    // MARK: What is shown

    func testATextIsOfferedByItsFirstLine() {
        XCTAssertEqual(ClipboardSuggestions.forText("  Bonjour\n", concealed: false), .text(preview: "Bonjour", isLink: false))
        XCTAssertEqual(ClipboardSuggestions.forText("\n\nligne un\nligne deux", concealed: false), .text(preview: "ligne un…", isLink: false))
        let long = String(repeating: "mot ", count: 80)
        guard case let .text(preview, _)? = ClipboardSuggestions.forText(long, concealed: false) else { return XCTFail("a text") }
        XCTAssertLessThanOrEqual(preview.count, ClipboardSuggestions.maxPreview + 1)
        XCTAssertTrue(preview.hasSuffix("…"))
        XCTAssertNil(ClipboardSuggestions.forText("  \n\t", concealed: false), "nothing to offer")
    }

    func testALinkIsRecognised() {
        XCTAssertEqual(ClipboardSuggestions.forText("https://example.org/a?b=1", concealed: false), .text(preview: "https://example.org/a?b=1", isLink: true))
        XCTAssertEqual(ClipboardSuggestions.forText("voir https://example.org", concealed: false), .text(preview: "voir https://example.org", isLink: false))
        XCTAssertEqual(ClipboardSuggestions.forText("mailto:a@b.c", concealed: false), .text(preview: "mailto:a@b.c", isLink: false))
    }

    func testAPasswordIsOfferedButNeverShown() {
        XCTAssertEqual(ClipboardSuggestions.forText("un texte banal", concealed: true), .hidden, "marked by a password manager")
        XCTAssertEqual(ClipboardSuggestions.forText("Kx9#mP2$vL7q!", concealed: false), .hidden, "it looks like one")
    }

    func testAPasswordOnTheFirstOfSeveralLinesIsNotShownEither() {
        XCTAssertEqual(ClipboardSuggestions.forText("Kx9#mP2$vL7q!\nmoi@example.org", concealed: false), .hidden)
        XCTAssertEqual(ClipboardSuggestions.forText("Bonjour à tous\nKx9#mP2$vL7q!", concealed: false), .text(preview: "Bonjour à tous…", isLink: false))
    }

    func testWhatLandsOnTheClipboardWhileTomiIsInFrontIsNotOffered() {
        var clock = ClipboardClock()
        clock.observe(count: 1, now: t0)
        clock.observe(count: 2, now: t0.addingTimeInterval(1))
        XCTAssertTrue(clock.isFresh(now: t0.addingTimeInterval(2)))
        // Tomi in front, the clipboard changes (a copy in Tomi, another device): the earlier offer is over too.
        clock.ignoreChange(count: 3)
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(3)))
        clock.observe(count: 3, now: t0.addingTimeInterval(4))
        XCTAssertFalse(clock.isFresh(now: t0.addingTimeInterval(4)))
        // Tomi in front with nothing new: the offer made before stays.
        clock.observe(count: 4, now: t0.addingTimeInterval(5))
        clock.ignoreChange(count: 4)
        XCTAssertTrue(clock.isFresh(now: t0.addingTimeInterval(6)))
    }

    func testOnlyAReadSuggestionCanBeSentAsItIs() {
        XCTAssertTrue(ClipboardSuggestion.text(preview: "a", isLink: false).isRead)
        XCTAssertTrue(ClipboardSuggestion.files(names: ["a.pdf"]).isRead)
        XCTAssertTrue(ClipboardSuggestion.hidden.isRead)
        XCTAssertFalse(ClipboardSuggestion.unread(.link).isRead, "iOS: nothing is sent that the person has not seen")
    }

    // MARK: Only an empty draft

    func testNothingIsOfferedOverADraft() {
        XCTAssertTrue(ClipboardSuggestions.canOffer(title: " ", text: "", attachments: 0, sending: false, justSent: false))
        XCTAssertFalse(ClipboardSuggestions.canOffer(title: "t", text: "", attachments: 0, sending: false, justSent: false))
        XCTAssertFalse(ClipboardSuggestions.canOffer(title: "", text: "x", attachments: 0, sending: false, justSent: false))
        XCTAssertFalse(ClipboardSuggestions.canOffer(title: "", text: "", attachments: 1, sending: false, justSent: false))
        XCTAssertFalse(ClipboardSuggestions.canOffer(title: "", text: "", attachments: 0, sending: true, justSent: false))
        XCTAssertFalse(ClipboardSuggestions.canOffer(title: "", text: "", attachments: 0, sending: false, justSent: true))
    }
}
