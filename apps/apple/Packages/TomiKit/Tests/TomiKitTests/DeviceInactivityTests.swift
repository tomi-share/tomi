import XCTest
@testable import TomiKit

/// The hint that suggests removing another device that has not shown up for longer than the relay's
/// `inactive_after_days` (Android `DeviceInactivityTest.kt`, case for case).
final class DeviceInactivityTests: XCTestCase {
    private let now = ISO8601DateFormatter().date(from: "2026-11-09T12:00:00Z")!
    private let day: TimeInterval = 24 * 60 * 60

    private func other(_ seen: Date?) -> Device {
        Device(id: "d", name: "PC-Maison", platform: .windows, lastActivity: seen, isThisDevice: false)
    }

    private func days(_ device: Device, thresholdDays: Int = 30) -> Int? {
        DeviceInactivity.inactiveDays(device, now: now, thresholdDays: thresholdDays, lastSeenFromRelay: true)
    }

    func testExactlyAtTheThresholdIsStillActive() {
        XCTAssertNil(days(other(now.addingTimeInterval(-30 * day))))
    }

    func testOneMillisecondPastTheThresholdIsInactive() {
        XCTAssertEqual(days(other(now.addingTimeInterval(-30 * day - 0.001))), 30)
    }

    func testCountsWholeDays() {
        XCTAssertEqual(days(other(now.addingTimeInterval(-34 * day))), 34)
        XCTAssertEqual(days(other(now.addingTimeInterval(-34 * day - 12 * 3600))), 34)
    }

    func testARecentDeviceIsActive() {
        XCTAssertNil(days(other(now)))
        XCTAssertNil(days(other(now.addingTimeInterval(-29 * day))))
    }

    func testADeviceThatNeverShowedUpIsNotReported() {
        XCTAssertNil(days(other(nil)))
    }

    func testThisDeviceIsNeverReported() {
        let me = Device(id: "me", name: "iPhone", platform: .ios, lastActivity: now.addingTimeInterval(-400 * day), isThisDevice: true)
        XCTAssertNil(days(me))
        XCTAssertNil(days(Device(id: "me", name: "iPhone", platform: .ios, lastActivity: nil, isThisDevice: true)))
    }

    func testADateInTheFutureIsNotInactive() {
        XCTAssertNil(days(other(now.addingTimeInterval(5 * day))))
    }

    func testTheThresholdIsTheOneGiven() {
        let device = other(now.addingTimeInterval(-10 * day))
        XCTAssertNil(days(device, thresholdDays: 10))
        XCTAssertEqual(days(device, thresholdDays: 9), 10)
    }

    func testTheDefaultThresholdIsTheRelaysThirtyDays() {
        XCTAssertEqual(DeviceInactivity.inactiveAfterDays, 30)
    }

    func testNothingIsReportedWhileTheDateIsNotTheRelaysLastSeen() {
        let old = other(now.addingTimeInterval(-400 * day))
        XCTAssertNil(DeviceInactivity.inactiveDays(old, now: now, lastSeenFromRelay: false))
    }
}
