import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/common/Formatting.kt.
// TomiKit cannot read the app's strings: labels are returned as structured values (enums carrying numbers and
// names, with the Android string resource key they stand for), and the app maps them to its own localisation.
// Dates and sizes, which need no app string, are formatted here in French (`fr_FR`). Time is injected.

private let frenchLocale = Locale(identifier: "fr_FR")

/// Compact timestamp of a card.
public enum TimestampLabel: Sendable, Hashable {
    /// Time today, e.g. "14:02".
    case time(String)
    /// Yesterday (resource `time_yesterday`, French "Hier"; lower case "hier" inside a sentence).
    case yesterday
    /// Short date, e.g. "6 nov.".
    case date(String)

    /// French text, with "hier" in lower case as the spec shows it.
    public var frenchText: String {
        switch self {
        case let .time(value), let .date(value): value
        case .yesterday: "hier"
        }
    }
}

/// Compact timestamp: time today, "yesterday", otherwise the short date. Days are those of `calendar` (its time zone).
public func formatTimestamp(_ date: Date, now: Date, calendar: Calendar = .current) -> TimestampLabel {
    if calendar.isDate(date, inSameDayAs: now) {
        return .time(formatted(date, "HH:mm", calendar))
    }
    if let yesterday = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: yesterday) {
        return .yesterday
    }
    return .date(formatted(date, "d MMM", calendar))
}

/// Full date and time for the detail screen, e.g. "mer. 5 nov. 13:47".
public func formatDateTime(_ date: Date, calendar: Calendar = .current) -> String {
    formatted(date, "EEE d MMM HH:mm", calendar)
}

/// Exact send date for the detail screen, with seconds, e.g. "5 novembre 2026 à 13:47:09".
public func formatPreciseDateTime(_ date: Date, calendar: Calendar = .current) -> String {
    formatted(date, "d MMMM yyyy 'à' HH:mm:ss", calendar)
}

/// Always Gregorian (like Android), in the time zone of `calendar`; the user's calendar only decides "today" and
/// "yesterday". Formatters are cached (one per pattern and time zone).
private func formatted(_ date: Date, _ pattern: String, _ calendar: Calendar) -> String {
    FormatterCache.shared.formatter(pattern: pattern, timeZone: calendar.timeZone).string(from: date)
}

private final class FormatterCache: @unchecked Sendable {
    static let shared = FormatterCache()
    private let lock = NSLock()
    private var formatters: [String: DateFormatter] = [:]

    func formatter(pattern: String, timeZone: TimeZone) -> DateFormatter {
        lock.withLock {
            let key = pattern + "|" + timeZone.identifier
            if let cached = formatters[key] { return cached }
            let formatter = DateFormatter()
            formatter.locale = frenchLocale
            formatter.calendar = Calendar(identifier: .gregorian)
            formatter.timeZone = timeZone
            formatter.dateFormat = pattern
            formatters[key] = formatter
            return formatter
        }
    }
}

/// Short file size in the style of Android's `Formatter.formatShortFileSize` (decimal units, French symbols):
/// "512 o", "18 ko", "2,4 Mo", "1,0 Go". Less than 10 units keep one decimal (two below 1), more than that none.
public func formatFileSize(_ bytes: Int64) -> String {
    let units = ["o", "ko", "Mo", "Go", "To", "Po"]
    var value = Double(max(bytes, 0))
    var unit = 0
    while value > 900 && unit < units.count - 1 {
        value /= 1000
        unit += 1
    }
    let digits: Int
    if unit == 0 {
        digits = 0
    } else if value < 1 {
        digits = 2
    } else if value < 10 {
        digits = 1
    } else {
        digits = 0
    }
    let factor = pow(10.0, Double(digits))
    let rounded = (value * factor).rounded(.toNearestOrAwayFromZero) / factor
    return String(format: "%.\(digits)f", locale: frenchLocale, rounded) + " " + units[unit]
}

/// Remaining lifetime of an item, as the plural resource it is shown with (`remaining_days`, `remaining_hours`,
/// `remaining_minutes`; the count is also the quantity argument).
public enum RemainingTime: Sendable, Hashable {
    case days(Int)
    case hours(Int)
    case minutes(Int)

    public var resourceKey: String {
        switch self {
        case .days: "remaining_days"
        case .hours: "remaining_hours"
        case .minutes: "remaining_minutes"
        }
    }

    public var count: Int {
        switch self {
        case let .days(n), let .hours(n), let .minutes(n): n
        }
    }
}

/// Remaining lifetime, whole days first, then whole hours, then minutes (at least 1); nil if there is no expiry or it is over.
public func formatRemaining(_ expiresAt: Date?, now: Date) -> RemainingTime? {
    guard let expiresAt else { return nil }
    let seconds = expiresAt.timeIntervalSince(now)
    if seconds <= 0 { return nil }
    let days = Int(seconds / 86_400)
    if days >= 1 { return .days(days) }
    let hours = Int(seconds / 3600)
    if hours >= 1 { return .hours(hours) }
    return .minutes(max(1, Int(seconds / 60)))
}

extension ExpiryChoice {
    /// Android string resource of the chip (`expiryLabel`).
    public var resourceKey: String {
        switch self {
        case .hour1: "expiry_hour1"
        case .day1: "expiry_day1"
        case .day7: "expiry_day7"
        case .day30: "expiry_day30"
        case .never: "expiry_never"
        case .afterFirstRead: "expiry_after_read"
        }
    }
}

public func expiryLabel(_ choice: ExpiryChoice) -> String { choice.resourceKey }

/// Delivery line of an outgoing item, as the Android string resource it is shown with and its format arguments.
public enum DeliveryLabel: Sendable, Hashable {
    case received
    /// Waiting to be sent (`isOnline`) or waiting for the network.
    case queued
    case waitingNetwork
    case sent
    case failed
    case deliveredAll
    case deliveredPartial(delivered: Int, total: Int)
    /// `devices` are the names joined with ", ".
    case expired(delivered: Int, devices: String)

    public var resourceKey: String {
        switch self {
        case .received: "status_received"
        case .queued: "status_queued"
        case .waitingNetwork: "status_waiting_network"
        case .sent: "status_sent"
        case .failed: "status_failed"
        case .deliveredAll: "status_delivered_all"
        case .deliveredPartial: "status_delivered_partial"
        case .expired: "status_expired"
        }
    }

    /// Format arguments (`%1$d`, `%2$d` / `%2$s`) in order, as strings.
    public var arguments: [String] {
        switch self {
        case let .deliveredPartial(delivered, total): [String(delivered), String(total)]
        case let .expired(delivered, devices): [String(delivered), devices]
        default: []
        }
    }
}

public func deliveryLabel(_ status: DeliveryStatus, isOnline: Bool) -> DeliveryLabel {
    switch status {
    case .received: .received
    case .queued: isOnline ? .queued : .waitingNetwork
    case .sent: .sent
    case .failed: .failed
    case let .delivered(delivered, total):
        delivered == total ? .deliveredAll : .deliveredPartial(delivered: delivered, total: total)
    case let .expiredBeforeDelivery(delivered, deviceNames):
        .expired(delivered: delivered, devices: deviceNames.joined(separator: ", "))
    }
}
