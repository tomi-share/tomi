package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.ReadPolicy
import java.time.Duration
import java.time.Instant

/** Expiry values offered in the chips of the send screen and of the options sheet. */
enum class ExpiryChoice(val duration: Duration?) {
    Never(null),
    Hour1(Duration.ofHours(1)),
    Day1(Duration.ofDays(1)),
    Day7(Duration.ofDays(7)),
    Day30(Duration.ofDays(30)),
    AfterFirstRead(null),
}

data class ResolvedExpiry(val expiresAt: Instant?, val readPolicy: ReadPolicy)

/**
 * Product decision: nothing expires unless the user explicitly asks for it, secrets included. The default is
 * [ExpiryChoice.Never] for every item; a duration or "after first read" is an opt-in chosen per item, and the app
 * never adds an expiry of its own (no hidden cap, no stricter default for secrets).
 */
object ExpiryPolicy {

    /** Default for every item, secret or not. */
    val DEFAULT: ExpiryChoice = ExpiryChoice.Never

    /**
     * Choices offered, default first: Never, [After first read, hidden content without attachment only], 1 h, 1 d, 7 d, 30 d
     * (no 5 min, product decision). "After first read" is only offered:
     * - for a hidden item: for other content, merely opening the card marks it read, which would erase it as soon as it is seen;
     * - for an item WITHOUT attachment ([hasAttachment] false): "after first read" destroys the item once it is consulted, so
     *   an attached file would be lost right after "Afficher" (product decision).
     */
    fun optionsFor(secret: Boolean, hasAttachment: Boolean): List<ExpiryChoice> = buildList {
        add(ExpiryChoice.Never)
        if (secret && !hasAttachment) add(ExpiryChoice.AfterFirstRead)
        addAll(listOf(ExpiryChoice.Hour1, ExpiryChoice.Day1, ExpiryChoice.Day7, ExpiryChoice.Day30))
    }

    fun isAllowed(choice: ExpiryChoice, secret: Boolean, hasAttachment: Boolean): Boolean = choice in optionsFor(secret, hasAttachment)

    /**
     * Called when the "secret" switch or the attachments change. The user's choice is kept when it is still allowed; turning a
     * secret into plain content, or attaching a file, drops "after first read" back to [DEFAULT]. Turning the switch on never
     * adds an expiry.
     */
    fun onContentChanged(current: ExpiryChoice, secret: Boolean, hasAttachment: Boolean): ExpiryChoice =
        if (isAllowed(current, secret, hasAttachment)) current else DEFAULT

    /** A choice that is not allowed for this content falls back to [DEFAULT], i.e. no expiry. */
    fun resolve(choice: ExpiryChoice, secret: Boolean, hasAttachment: Boolean, now: Instant): ResolvedExpiry {
        val safe = if (isAllowed(choice, secret, hasAttachment)) choice else DEFAULT
        return when (safe) {
            // Read once, then gone everywhere (tombstone). No time limit is added on top of what the user chose.
            ExpiryChoice.AfterFirstRead -> ResolvedExpiry(null, ReadPolicy.AfterFirstRead)
            else -> ResolvedExpiry(safe.duration?.let { now.plus(it) }, ReadPolicy.Normal)
        }
    }
}
