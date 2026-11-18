import Foundation
import TomiKit

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/send/SendCoordinator.kt.

/// Schedules and cancels the dispatch of a queued item (in the app for now; background tasks in PR-10).
@MainActor
public protocol SendScheduling: AnyObject {
    /// Replaces any pending dispatch for `itemId`. The item must not leave before `deadline`.
    func schedule(itemId: String, deadline: UndoDeadline)
    /// Schedules a dispatch only if none is pending (resume after a restart), never before `createdAt` plus the undo
    /// window (the app may have been closed during it).
    func ensureScheduled(itemId: String, createdAt: Date)
    func cancel(itemId: String)
}

public struct StagedAttachments: Sendable {
    public var attachments: [DraftAttachment]
    /// Files left out: unreadable, or over the limits.
    public var skipped: Int

    public init(attachments: [DraftAttachment], skipped: Int) {
        self.attachments = attachments
        self.skipped = skipped
    }
}

/// Copies the chosen files where the core can read them from a plain path.
public protocol AttachmentStaging: Sendable {
    func stage(draftId: String, files: [SharedFile]) async -> StagedAttachments
    /// The previews the attachments carry (never asked for a hidden item).
    func thumbnails(_ attachments: [DraftAttachment]) async -> [DraftAttachment]
    func cleanup(draftId: String)
}

extension AttachmentStaging {
    public func thumbnails(_ attachments: [DraftAttachment]) async -> [DraftAttachment] { attachments }
}

/// What the screen keeps after a send to offer "Annuler" and "Options".
public struct Submission: Sendable, Hashable {
    public let id: String
    public let parsed: ParsedShare
    public let secret: Bool
    public let expiry: ExpiryChoice
    public let skippedFiles: Int
    /// End of the undo window; the banner and the scheduler share it.
    public let deadline: UndoDeadline
}

public enum CancelOutcome: Sendable, Hashable {
    /// The item had not left the device: it is gone, and the draft can be edited again.
    case cancelled
    /// The item already left (or is leaving): nothing was deleted, the user must be told.
    case tooLate
}

/// Single entry point for sending: copy the files, hand the draft to the core's outbox, then schedule the real dispatch
/// after the undo window. Used by the send screen, and later by the share extension (PR-8).
@MainActor
public final class SendCoordinator {
    public static let undoWindowMs: Int64 = 3_000

    private let core: any TomiCore
    private let scheduler: SendScheduling
    private let staging: AttachmentStaging
    private let now: () -> Date
    private let monotonicMs: () -> Int64
    private let newId: () -> String

    public init(
        core: any TomiCore,
        scheduler: SendScheduling,
        staging: AttachmentStaging,
        now: @escaping () -> Date = Date.init,
        monotonicMs: @escaping () -> Int64 = MonotonicClock.nowMs,
        newId: @escaping () -> String = { UUID().uuidString.lowercased() }
    ) {
        self.core = core
        self.scheduler = scheduler
        self.staging = staging
        self.now = now
        self.monotonicMs = monotonicMs
        self.newId = newId
    }

    public func submit(_ parsed: ParsedShare, secret: Bool, expiry: ExpiryChoice, undoWindowMs: Int64 = SendCoordinator.undoWindowMs) async throws -> Submission {
        let id = newId()
        // The core has read the files when `enqueue` returns (or it failed): our copies go either way.
        defer { staging.cleanup(draftId: id) }
        let staged = await staging.stage(draftId: id, files: parsed.files)
        // A hidden item carries none: no file of it is even handed to Quick Look for one.
        let attachments = secret ? staged.attachments : await staging.thumbnails(staged.attachments)
        let draft = OutgoingDraftFactory.create(id: id, parsed: parsed, attachments: attachments, secret: secret, expiry: expiry, now: now())
        try await core.enqueue(draft)
        let deadline = UndoDeadline.startingAt(monotonicMs(), windowMs: undoWindowMs)
        scheduler.schedule(itemId: id, deadline: deadline)
        return Submission(id: id, parsed: parsed, secret: secret, expiry: expiry, skippedFiles: staged.skipped, deadline: deadline)
    }

    /// "Annuler". Only the core knows whether the item already left, so it decides, atomically (`cancelIfQueued`). The
    /// scheduled dispatch stops only once the item is really cancelled; too late, nothing is deleted: the item stays in
    /// the feed, where the user can delete it everywhere.
    public func cancel(_ submission: Submission) async throws -> CancelOutcome {
        guard try await core.cancelIfQueued(id: submission.id) else { return .tooLate }
        scheduler.cancel(itemId: submission.id)
        return .cancelled
    }

    /// At start-up: items of this device still waiting for the relay get a dispatch again (the app may have been closed
    /// during the undo window or offline).
    public func resumeQueued(localDeviceId: String?) {
        guard let localDeviceId else { return }
        for item in core.state.items where item.isOutgoing(localDeviceId: localDeviceId) && item.deliveries.contains(where: { $0.state == .queued }) {
            scheduler.ensureScheduled(itemId: item.id, createdAt: item.createdAt)
        }
    }
}
