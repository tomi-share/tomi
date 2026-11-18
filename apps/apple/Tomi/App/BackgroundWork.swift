import BackgroundTasks
import Foundation
import TomiKit

/// Work iOS lets the app do while it is not on screen (spec 12): a refresh (`eu.shaffe.tomi.refresh`, a sync, at best
/// every 15 minutes) and the sending of what is left queued (`eu.shaffe.tomi.send`, once the network is there). iOS
/// decides when; until the doorbell push exists, this is how an item arrives or leaves with the app closed.
enum BackgroundWork {
    static let refreshId = "eu.shaffe.tomi.refresh"
    static let sendId = "eu.shaffe.tomi.send"
    static let refreshInterval: TimeInterval = 15 * 60

    /// Registers the handlers; must run before the end of the launch (`didFinishLaunching`).
    static func register() {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: refreshId, using: nil) { task in
            run(task, sendOnly: false)
        }
        BGTaskScheduler.shared.register(forTaskWithIdentifier: sendId, using: nil) { task in
            run(task, sendOnly: true)
        }
    }

    /// Asks for the next refresh (and for a sending when something waits): called when the app goes to the background.
    static func schedule(hasQueued: Bool) {
        let refresh = BGAppRefreshTaskRequest(identifier: refreshId)
        refresh.earliestBeginDate = Date(timeIntervalSinceNow: refreshInterval)
        try? BGTaskScheduler.shared.submit(refresh)
        guard hasQueued else { return }
        let send = BGProcessingTaskRequest(identifier: sendId)
        send.requiresNetworkConnectivity = true
        send.requiresExternalPower = false
        try? BGTaskScheduler.shared.submit(send)
    }

    private static func run(_ task: BGTask, sendOnly: Bool) {
        // Completed once: by the work, or at once when iOS takes the time back (a blocking core call may outlast it).
        let completion = Completion(task)
        let work = Task { @MainActor in
            let done = await AppContainer.current?.runInBackground(sendOnly: sendOnly) ?? false
            // The next refresh, and a new sending while something still waits.
            schedule(hasQueued: AppContainer.current?.hasQueuedSends ?? false)
            completion.finish(done)
        }
        task.expirationHandler = {
            work.cancel()
            completion.finish(false)
        }
    }

    private final class Completion: @unchecked Sendable {
        private let task: BGTask
        private let lock = NSLock()
        private var finished = false

        init(_ task: BGTask) { self.task = task }

        func finish(_ success: Bool) {
            lock.lock()
            defer { lock.unlock() }
            guard !finished else { return }
            finished = true
            task.setTaskCompleted(success: success)
        }
    }
}
