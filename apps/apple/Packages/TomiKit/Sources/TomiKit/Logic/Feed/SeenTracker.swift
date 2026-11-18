import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/SeenTracker.kt.
// The Android `visibleKeys(LazyListLayoutInfo)` has no counterpart: the feed asks each card whether it is on screen
// enough (`visibleEnough`) and tells the tracker the set of the ones that are.

/// Reading by looking: a card that stays on screen for `dwellMs` counts as seen. A quick scroll past reads nothing.
///
/// Pure bookkeeping (no clock, no view): the caller says which keys are on screen and when, and asks which ones are
/// due. A key that was reported as due is not due again until it has left the screen.
public struct SeenTracker: Sendable {
    /// The same dwell as the Windows and Android feeds.
    public static let defaultDwellMs: Int64 = 1_500

    /// Share of a card (or of the visible zone, for a taller card) that must be on screen.
    public static let visibleShare: Double = 0.6

    private let dwellMs: Int64
    private var since: [String: Int64] = [:]
    private var reported: Set<String> = []

    public init(dwellMs: Int64 = SeenTracker.defaultDwellMs) {
        self.dwellMs = dwellMs
    }

    /// The keys that are on screen now (replaces the previous answer).
    public mutating func update(onScreen: Set<String>, now: Int64) {
        since = since.filter { onScreen.contains($0.key) }
        reported.formIntersection(onScreen)
        for key in onScreen where since[key] == nil {
            since[key] = now
        }
    }

    /// The keys that stayed long enough, once each (sorted, for a stable order).
    public mutating func takeDue(now: Int64) -> [String] {
        let due = since.filter { !reported.contains($0.key) && now - $0.value >= dwellMs }.keys.sorted()
        reported.formUnion(due)
        return due
    }

    /// Time left before the next key is due, or nil when none is waiting.
    public func nextDelay(now: Int64) -> Int64? {
        since.filter { !reported.contains($0.key) }.values.map { max($0 + dwellMs - now, 0) }.min()
    }

    /// Everything is forgotten: the screen was not attended (paused, covered), the time did not count.
    public mutating func reset() {
        since.removeAll()
        reported.removeAll()
    }

    /// Whether a row is on screen enough. `offset` and `size` are the row, `top` and `bottom` the free zone (the
    /// header and the toolbar float over the list), all in points along the scroll axis. A row taller than the zone
    /// needs the same share of the zone instead of the share of itself.
    public static func visibleEnough(
        offset: Double, size: Double, top: Double, bottom: Double, share: Double = SeenTracker.visibleShare
    ) -> Bool {
        let zone = bottom - top
        if size <= 0 || zone <= 0 { return false }
        let shown = max(min(offset + size, bottom) - max(offset, top), 0)
        // A small tolerance: 60 % of 200 must count, whatever the rounding of the floating point.
        return shown / min(size, zone) >= share - 0.001
    }
}
