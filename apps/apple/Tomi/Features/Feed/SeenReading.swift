import Foundation
import TomiKit

/// Reads the cards the user looks at (Android `ReadWhenSeenEffect`): a card on screen enough (`SeenTracker.visibleEnough`)
/// for `SeenTracker.defaultDwellMs` is reported once through `onSeen`. The feed tells where each card is; this works out
/// which ones are on screen, and drives the tracker.
///
/// Only while the feed is attended (`setAttended`: the app active and the feed the screen on top). Otherwise nothing is
/// reported and the time spent there does not count: the tracker is emptied, and starts again when the feed is back.
@MainActor
final class SeenReading {
    /// Called with the ids of the cards that stayed long enough, once each.
    var onSeen: @MainActor ([String]) -> Void = { _ in }

    private var tracker = SeenTracker()
    /// Last known position of each card, in points from the top of the scroll view.
    private var frames: [String: (minY: Double, height: Double)] = [:]
    /// Cards whose view is in the hierarchy (a covered feed, a card recycled by the lazy stack: not).
    private var present: Set<String> = []
    private var viewportHeight: Double = 0
    /// Room at the bottom of the scroll view taken by the floating bar (the cards pass under it).
    private var clearance: Double = 0
    private var onScreen: Set<String> = []
    private var attended = false
    private var waiting: Task<Void, Never>?

    func setFrame(_ id: String, minY: Double, height: Double) {
        frames[id] = (minY, height)
        if refresh(id) { restart() }
    }

    func appeared(_ id: String) {
        present.insert(id)
        if refresh(id) { restart() }
    }

    func disappeared(_ id: String) {
        present.remove(id)
        if refresh(id) { restart() }
    }

    func setViewport(height: Double, clearance: Double) {
        guard height != viewportHeight || clearance != self.clearance else { return }
        viewportHeight = height
        self.clearance = clearance
        var changed = false
        for id in frames.keys where refresh(id) { changed = true }
        if changed { restart() }
    }

    func setAttended(_ attended: Bool) {
        guard attended != self.attended else { return }
        self.attended = attended
        if attended {
            restart()
        } else {
            waiting?.cancel()
            waiting = nil
            tracker.reset()
        }
    }

    /// Puts `id` in or out of `onScreen`; true when that changed.
    private func refresh(_ id: String) -> Bool {
        var visible = false
        if present.contains(id), let frame = frames[id] {
            visible = SeenTracker.visibleEnough(offset: frame.minY, size: frame.height, top: 0, bottom: viewportHeight - clearance)
        }
        return visible ? onScreen.insert(id).inserted : onScreen.remove(id) != nil
    }

    /// Tells the tracker what is on screen now and waits for the next card to be due (Android `collectLatest`).
    private func restart() {
        guard attended else { return }
        tracker.update(onScreen: onScreen, now: MonotonicClock.nowMs())
        waiting?.cancel()
        waiting = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, let delay = self.tracker.nextDelay(now: MonotonicClock.nowMs()) else { return }
                try? await Task.sleep(for: .milliseconds(delay))
                if Task.isCancelled { return }
                let due = self.tracker.takeDue(now: MonotonicClock.nowMs())
                if !due.isEmpty { self.onSeen(due) }
            }
        }
    }
}
