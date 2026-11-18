import Foundation
import TomiKit
#if canImport(UIKit)
import UIKit
#endif

/// Dispatch while the app runs (Android `WorkManagerSendScheduler`): a task per item waits out the undo window on the
/// monotonic clock, then pushes the item; "retry later" (no network, relay error) tries again with an exponential
/// back-off from 15 s. Each task asks iOS for a little background time, so a send still leaves when the user switches
/// app right after "Envoyer". Background tasks (`BGProcessingTask`) take over in PR-10; until then, `ensureScheduled`
/// at start-up resumes what the app left queued.
@MainActor
public final class InAppSendScheduler: SendScheduling {
    public static let firstBackoff: Duration = .seconds(15)
    public static let maxBackoff: Duration = .seconds(15 * 60)

    private let dispatch: @MainActor (String) async -> DispatchResult
    private let monotonicMs: () -> Int64
    private let now: () -> Date
    private let sleep: (Duration) async throws -> Void
    /// Asks the system to keep the app running for a while; the returned closure ends it.
    private let keepAlive: @MainActor () -> (@MainActor () -> Void)
    private var tasks: [String: (token: UUID, task: Task<Void, Never>)] = [:]

    /// `dispatch` is `TomiCore.dispatch`, a failure counting as "retry later".
    public init(
        core: any TomiCore,
        monotonicMs: @escaping () -> Int64 = MonotonicClock.nowMs,
        now: @escaping () -> Date = Date.init,
        sleep: @escaping (Duration) async throws -> Void = { try await Task.sleep(for: $0) },
        keepAlive: (@MainActor () -> (@MainActor () -> Void))? = nil
    ) {
        self.dispatch = { [weak core] id in
            guard let core else { return .retryLater }
            return (try? await core.dispatch(id: id)) ?? .retryLater
        }
        self.monotonicMs = monotonicMs
        self.now = now
        self.sleep = sleep
        self.keepAlive = keepAlive ?? Self.backgroundTime
    }

    /// A background task of UIKit (about 30 s): enough for the window and one dispatch.
    private static func backgroundTime() -> (@MainActor () -> Void) {
        #if canImport(UIKit)
        var id = UIBackgroundTaskIdentifier.invalid
        id = UIApplication.shared.beginBackgroundTask(withName: "tomi.send") {
            UIApplication.shared.endBackgroundTask(id)
            id = .invalid
        }
        return {
            guard id != .invalid else { return }
            UIApplication.shared.endBackgroundTask(id)
            id = .invalid
        }
        #else
        return {}
        #endif
    }

    public func schedule(itemId: String, deadline: UndoDeadline) {
        start(itemId, waitMs: deadline.waitBeforeDispatchMs(monotonicMs()))
    }

    public func ensureScheduled(itemId: String, createdAt: Date) {
        guard tasks[itemId] == nil else { return }
        let window = SendCoordinator.undoWindowMs
        let left = Int64((createdAt.timeIntervalSince(now()) * 1000).rounded()) + window
        start(itemId, waitMs: min(max(left, 0), window))
    }

    public func cancel(itemId: String) {
        tasks.removeValue(forKey: itemId)?.task.cancel()
    }

    /// Items with a pending dispatch (tests, diagnostics).
    public var pending: Set<String> { Set(tasks.keys) }

    private func start(_ itemId: String, waitMs: Int64) {
        tasks[itemId]?.task.cancel()
        let sleep = self.sleep
        let dispatch = self.dispatch
        let token = UUID()
        let release = keepAlive()
        let task = Task { [weak self] in
            defer {
                release()
                self?.finished(itemId, token: token)
            }
            do {
                if waitMs > 0 { try await sleep(.milliseconds(waitMs)) }
                var backoff = Self.firstBackoff
                while true {
                    try Task.checkCancellation()
                    switch await dispatch(itemId) {
                    case .sent, .notFound, .failed:
                        return
                    case .retryLater:
                        try await sleep(backoff)
                        backoff = min(backoff * 2, Self.maxBackoff)
                    }
                }
            } catch {
                // Cancelled ("Annuler", or replaced by a new schedule): the item stays where the core left it.
            }
        }
        tasks[itemId] = (token, task)
    }

    private func finished(_ itemId: String, token: UUID) {
        // A newer schedule for the same item may have replaced this task: only this one's entry is removed.
        if tasks[itemId]?.token == token { tasks.removeValue(forKey: itemId) }
    }
}
