import Foundation
import TomiKit
import TomiRuntime

/// Sends what the share extension put in the inbox of the app group (section 10): each share past the end of its undo
/// window is claimed (a late "Annuler" then finds it gone), handed to the core like a send of the send screen with no
/// window left, and its directory removed. Runs when the core is ready, when the app comes back, and when the extension
/// says it put something there (iPad side by side); a share still in its window is looked at again at its end.
@MainActor
final class ShareIntake {
    /// A share whose end is further ahead than this was made before the clock was set back: sent at once.
    static let maxWindow: TimeInterval = 10

    private let inbox: ShareInbox
    private let coordinator: SendCoordinator
    private var running = false
    private var again = false
    private var next: Task<Void, Never>?
    var onChanged: (() -> Void)?
    /// A share could not be handed to the core; it stays in the inbox.
    var onFailed: (() -> Void)?
    /// Files of a share left out by the limits of the item.
    var onSkippedFiles: ((Int) -> Void)?
    /// Shares dropped because they waited longer than `ShareInbox.maxAge` (how many).
    var onExpired: ((Int) -> Void)?
    private var stopped = false

    init(inbox: ShareInbox, coordinator: SendCoordinator) {
        self.inbox = inbox
        self.coordinator = coordinator
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), Unmanaged.passUnretained(self).toOpaque(),
            { _, observer, _, _, _ in
                // Darwin notifications come on the main thread: no hop, the intake cannot be freed meanwhile.
                guard let observer else { return }
                let intake = Unmanaged<ShareIntake>.fromOpaque(observer).takeUnretainedValue()
                MainActor.assumeIsolated { intake.onChanged?() }
            },
            AppGroup.inboxChanged as CFString, nil, .deliverImmediately
        )
    }

    deinit {
        CFNotificationCenterRemoveEveryObserver(CFNotificationCenterGetDarwinNotifyCenter(), Unmanaged.passUnretained(self).toOpaque())
        next?.cancel()
    }

    /// Sends what is ready. A call while it runs makes it look once more at the end.
    func drain() async {
        guard !running else {
            again = true
            return
        }
        running = true
        defer { running = false }
        repeat {
            again = false
            guard !stopped else { return }
            let inbox = self.inbox
            let ready = await Task.detached(priority: .userInitiated) {
                inbox.ready(now: Date(), maxWindow: Self.maxWindow)
            }.value
            for handoff in ready {
                guard !stopped, let claimed = inbox.claim(handoff) else { continue }
                let parsed: ParsedShare
                do {
                    parsed = try inbox.parsed(handoff, in: claimed)
                } catch let error as ShareSealError where error == .authentication || error == .malformed {
                    // A sealed file that does not open (cut short, altered) never will: trying again would only fail
                    // again, so the share goes, and the user is told like for any share that could not be sent.
                    inbox.finish(claimed)
                    onFailed?()
                    continue
                } catch {
                    // Anything else (a file not readable right now): back to the inbox, tried again at the next look.
                    if !inbox.unclaim(claimed, id: handoff.id) { inbox.finish(claimed) }
                    onFailed?()
                    return
                }
                do {
                    // The extension held the undo window: none is left.
                    let sent = try await coordinator.submit(
                        parsed, secret: handoff.secret, expiry: handoff.expiry, undoWindowMs: 0
                    )
                    inbox.finish(claimed)
                    if sent.skippedFiles > 0 { onSkippedFiles?(sent.skippedFiles) }
                } catch {
                    // Back to the inbox, tried again at the next look (never lost without a word, never sent twice).
                    if !inbox.unclaim(claimed, id: handoff.id) { inbox.finish(claimed) }
                    onFailed?()
                    return
                }
            }
            scheduleNext()
        } while again
        // After the sending: what is still there past `ShareInbox.maxAge` could not be sent, and goes.
        let dropped = inbox.dropExpired(now: Date())
        if dropped > 0 { onExpired?(dropped) }
    }

    /// The core is replaced: this intake does nothing more.
    func stop() {
        stopped = true
        next?.cancel()
        next = nil
        CFNotificationCenterRemoveEveryObserver(CFNotificationCenterGetDarwinNotifyCenter(), Unmanaged.passUnretained(self).toOpaque())
    }

    private func scheduleNext() {
        next?.cancel()
        guard let date = inbox.nextReadyDate(now: Date(), maxWindow: Self.maxWindow) else { return }
        next = Task { [weak self] in
            try? await Task.sleep(for: .seconds(max(date.timeIntervalSinceNow, 0) + 0.1))
            guard !Task.isCancelled else { return }
            await self?.drain()
        }
    }
}
