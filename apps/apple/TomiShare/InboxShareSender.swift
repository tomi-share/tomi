import Foundation
import TomiKit

/// Hands a share to the app through the inbox of the app group: put there at once with the end of the undo window, taken
/// back by "Annuler". The app sends it after that end, the next time it runs (the extension never opens the core).
@MainActor
final class InboxShareSender: ShareSending {
    static let undoWindowMs: Int64 = 3_000

    private let inbox: ShareInbox
    /// Files the reading left out, shown by the banner.
    var skippedFiles = 0

    init(inbox: ShareInbox) {
        self.inbox = inbox
    }

    func submit(_ parsed: ParsedShare, secret: Bool, expiry: ExpiryChoice) async throws -> ShareSent {
        let inbox = self.inbox
        let window = TimeInterval(Self.undoWindowMs) / 1000
        let deadline = UndoDeadline.startingNow(windowMs: Self.undoWindowMs)
        let handoff = try await Task.detached(priority: .userInitiated) {
            try inbox.deposit(parsed, secret: secret, expiry: expiry, now: Date(), window: window)
        }.value
        // The app, if it runs (iPad side by side), looks at its inbox.
        CFNotificationCenterPostNotification(
            CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName(AppGroup.inboxChanged as CFString), nil, nil, true
        )
        return ShareSent(id: handoff.id, parsed: parsed, secret: secret, expiry: expiry, skippedFiles: skippedFiles, deadline: deadline)
    }

    func cancel(_ sent: ShareSent) async throws -> Bool {
        inbox.withdraw(id: sent.id)
    }
}
