import XCTest
@testable import TomiKit

// Port of apps/android/.../ui/common/Formatting.kt. Android has no unit test for it (it needs a Context and Compose
// resources), so these cases are new; they check the examples of docs/archive/apple-spec-implementation.md PR-4.
final class FormattingTests: XCTestCase {
    // The expected texts are the French ones of the charter.
    override func setUp() { TomiLocale.current = Locale(identifier: "fr_FR") }
    override func tearDown() { TomiLocale.current = .autoupdatingCurrent }

    func testAnotherLanguageTakesItsOwnForms() {
        TomiLocale.current = Locale(identifier: "en_US")
        XCTAssertEqual(formatFileSize(2_400_000), "2.4 MB")
        XCTAssertEqual(TimestampLabel.yesterday.text(yesterday: "Yesterday"), "yesterday")
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Europe/Paris")!
        let date = calendar.date(from: DateComponents(year: 2026, month: 11, day: 6, hour: 12))!
        let now = calendar.date(from: DateComponents(year: 2026, month: 11, day: 20, hour: 12))!
        XCTAssertEqual(formatTimestamp(date, now: now, calendar: calendar), .date("Nov 6"))
    }

    private var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Europe/Paris")!
        return calendar
    }

    /// 2026-11-06 at `hour:minute` in Paris (UTC+1 in November).
    private func paris(day: Int = 6, _ hour: Int, _ minute: Int, month: Int = 11) -> Date {
        calendar.date(from: DateComponents(year: 2026, month: month, day: day, hour: hour, minute: minute))!
    }

    func testTimestampIsTheTimeToday() {
        let now = paris(18, 0)
        XCTAssertEqual(formatTimestamp(paris(14, 2), now: now, calendar: calendar), .time("14:02"))
        XCTAssertEqual(formatTimestamp(paris(0, 5), now: now, calendar: calendar).text(yesterday: "Hier"), "00:05")
    }

    func testTimestampIsYesterdayThenTheShortDate() {
        let now = paris(0, 30)
        XCTAssertEqual(formatTimestamp(paris(day: 5, 23, 59), now: now, calendar: calendar), .yesterday)
        XCTAssertEqual(formatTimestamp(paris(day: 5, 23, 59), now: now, calendar: calendar).text(yesterday: "Hier"), "hier")
        XCTAssertEqual(formatTimestamp(paris(day: 4, 12, 0), now: now, calendar: calendar), .date("4 nov."))
        XCTAssertEqual(formatTimestamp(paris(day: 6, 12, 0, month: 10), now: now, calendar: calendar).text(yesterday: "Hier"), "6 oct.")
    }

    func testFullDateAndTime() {
        XCTAssertEqual(formatDateTime(paris(day: 5, 13, 47), calendar: calendar), "jeu. 5 nov. 13:47")
    }

    func testPreciseDateAndTimeHaveSecondsAndTheFullMonth() {
        let date = calendar.date(from: DateComponents(year: 2026, month: 11, day: 5, hour: 13, minute: 47, second: 9))!
        XCTAssertEqual(formatPreciseDateTime(date, calendar: calendar), "5 novembre 2026 à 13:47:09")
        // Midnight and a month with an accent, in the time zone of the calendar (UTC+2 in August).
        let night = calendar.date(from: DateComponents(year: 2026, month: 8, day: 1, hour: 0, minute: 5, second: 0))!
        XCTAssertEqual(formatPreciseDateTime(night, calendar: calendar), "1 août 2026 à 00:05:00")
    }

    func testFileSizesAreFrench() {
        XCTAssertEqual(formatFileSize(0), "0 o")
        XCTAssertEqual(formatFileSize(512), "512 o")
        XCTAssertEqual(formatFileSize(900), "900 o")
        XCTAssertEqual(formatFileSize(1500), "1,5 ko")
        XCTAssertEqual(formatFileSize(18_000), "18 ko")
        XCTAssertEqual(formatFileSize(2_400_000), "2,4 Mo")
        XCTAssertEqual(formatFileSize(25 * 1024 * 1024), "26 Mo")
        XCTAssertEqual(formatFileSize(1_000_000_000), "1,0 Go")
    }

    func testRemainingTime() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertNil(formatRemaining(nil, now: now))
        XCTAssertNil(formatRemaining(now, now: now))
        XCTAssertNil(formatRemaining(now.addingTimeInterval(-5), now: now))
        XCTAssertEqual(formatRemaining(now.addingTimeInterval(10), now: now), .minutes(1))
        XCTAssertEqual(formatRemaining(now.addingTimeInterval(59 * 60 + 59), now: now), .minutes(59))
        XCTAssertEqual(formatRemaining(now.addingTimeInterval(2 * 3600 + 100), now: now), .hours(2))
        XCTAssertEqual(formatRemaining(now.addingTimeInterval(24 * 3600), now: now), .days(1))
        XCTAssertEqual(formatRemaining(now.addingTimeInterval(3 * 86_400 + 3600), now: now), .days(3))
        XCTAssertEqual(RemainingTime.days(3).resourceKey, "remaining_days")
        XCTAssertEqual(RemainingTime.hours(2).count, 2)
        XCTAssertEqual(RemainingTime.minutes(1).resourceKey, "remaining_minutes")
    }

    func testExpiryLabelsAreTheAndroidResourceKeys() {
        XCTAssertEqual(
            ExpiryChoice.allCases.map(expiryLabel),
            ["expiry_never", "expiry_hour1", "expiry_day1", "expiry_day7", "expiry_day30", "expiry_after_read"]
        )
    }

    func testDeliveryLabels() {
        XCTAssertEqual(deliveryLabel(.received, isOnline: true), .received)
        XCTAssertEqual(deliveryLabel(.queued, isOnline: true), .queued)
        XCTAssertEqual(deliveryLabel(.queued, isOnline: false), .waitingNetwork)
        XCTAssertEqual(deliveryLabel(.sent, isOnline: false), .sent)
        XCTAssertEqual(deliveryLabel(.failed, isOnline: true), .failed)
        XCTAssertEqual(deliveryLabel(.delivered(delivered: 2, total: 2), isOnline: true), .deliveredAll)
        let partial = deliveryLabel(.delivered(delivered: 1, total: 3), isOnline: true)
        XCTAssertEqual(partial, .deliveredPartial(delivered: 1, total: 3))
        XCTAssertEqual(partial.resourceKey, "status_delivered_partial")
        XCTAssertEqual(partial.arguments, ["1", "3"])
        let expired = deliveryLabel(.expiredBeforeDelivery(delivered: 1, deviceNames: ["Tel", "PC"]), isOnline: true)
        XCTAssertEqual(expired.resourceKey, "status_expired")
        XCTAssertEqual(expired.arguments, ["1", "Tel, PC"])
        XCTAssertEqual(DeliveryLabel.sent.arguments, [])
    }
}
