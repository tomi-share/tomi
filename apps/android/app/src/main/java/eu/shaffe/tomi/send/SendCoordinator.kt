package eu.shaffe.tomi.send

import android.os.SystemClock
import eu.shaffe.tomi.core.DraftAttachment
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.share.ExpiryChoice
import eu.shaffe.tomi.share.OutgoingDraftFactory
import eu.shaffe.tomi.share.ParsedShare
import eu.shaffe.tomi.share.SharedStream
import java.time.Clock
import java.util.UUID

/** Schedules and cancels the background dispatch of a queued item (WorkManager in production). */
interface SendScheduling {
    /**
     * Replaces any pending dispatch for [itemId]. The job must not push the item before [deadline] (it waits for the
     * remaining time itself) and runs once the network is available.
     */
    fun schedule(itemId: String, deadline: UndoDeadline)

    /** Schedules a dispatch only if none is pending (used to resume after a restart). */
    fun ensureScheduled(itemId: String)

    fun cancel(itemId: String)
}

data class StagedAttachments(val attachments: List<DraftAttachment>, val skipped: Int)

/** Copies shared streams to private storage so the core can read them from a plain path. */
interface AttachmentStaging {
    suspend fun stage(draftId: String, streams: List<SharedStream>): StagedAttachments
    fun cleanup(draftId: String)
}

/** What the UI needs to keep to offer "Undo" and "Options" after a send. */
data class Submission(
    val id: String,
    val parsed: ParsedShare,
    val secret: Boolean,
    val expiry: ExpiryChoice,
    val skippedFiles: Int,
    /** End of the undo window; the banner and the background job share it. */
    val deadline: UndoDeadline,
)

/** Result of an undo. */
enum class CancelOutcome {
    /** The item had not left the device: it is gone and the draft can be edited again. */
    Cancelled,

    /** The item already left (or is leaving): nothing was deleted, the user must be told. */
    TooLate,
}

/**
 * Single entry point for sending: stage files, hand the draft to the core's outbox, then schedule the real
 * dispatch after the undo window. Used by the share target, the compose screen and the tile.
 */
class SendCoordinator(
    private val core: TomiCore,
    private val scheduler: SendScheduling,
    private val staging: AttachmentStaging,
    private val clock: Clock = Clock.systemUTC(),
    /** Monotonic real-time clock in ms; replaced in tests. */
    private val elapsedRealtimeMs: () -> Long = { SystemClock.elapsedRealtime() },
    private val newId: () -> String = { UUID.randomUUID().toString() },
) {
    suspend fun submit(
        parsed: ParsedShare,
        secret: Boolean,
        expiry: ExpiryChoice,
        undoWindowMs: Long = UNDO_WINDOW_MS,
    ): Submission {
        val id = newId()
        try {
            val staged = staging.stage(id, parsed.streams)
            val draft = OutgoingDraftFactory.create(id, parsed, staged.attachments, secret, expiry, clock.instant())
            core.enqueue(draft)
            val deadline = UndoDeadline.startingAt(elapsedRealtimeMs(), undoWindowMs)
            scheduler.schedule(id, deadline)
            return Submission(id, parsed, secret, expiry, staged.skipped, deadline)
        } finally {
            // The core has consumed the files when enqueue returns (or enqueue failed): drop our copies.
            staging.cleanup(id)
        }
    }

    /**
     * Undo. Only the core knows whether the item already left, so it decides, atomically ([TomiCore.cancelIfQueued]).
     * The scheduled job is stopped only once the item is really cancelled. If it is too late, nothing is deleted:
     * the item stays in the feed, where the user can delete it everywhere.
     */
    suspend fun cancel(submission: Submission): CancelOutcome {
        if (!core.cancelIfQueued(submission.id)) return CancelOutcome.TooLate
        scheduler.cancel(submission.id)
        return CancelOutcome.Cancelled
    }

    companion object {
        const val UNDO_WINDOW_MS = 3_000L
    }
}
