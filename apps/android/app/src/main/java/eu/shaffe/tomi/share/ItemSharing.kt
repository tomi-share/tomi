package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.security.AuthOutcome

/**
 * What goes into the Android `ACTION_SEND` intent when the user shares an item to another app.
 * [toString] never prints the content: a payload can hold a hidden item.
 *
 * [sensitive] is true for a hidden item: the intent then carries the text in a clip flagged `EXTRA_IS_SENSITIVE`, a neutral
 * `EXTRA_TITLE` and NO `EXTRA_SUBJECT` (the title of a hidden item is hidden too: it is never shared, see
 * [eu.shaffe.tomi.ui.common.buildShareIntent]). The clip flag and the title are a hypothesis to check on a device, not a
 * guarantee: `EXTRA_TITLE` does not replace `EXTRA_TEXT` (on Android 14+ the text preview of the sheet shows `EXTRA_TEXT`),
 * and whether the sheet's "Copy" button keeps the sensitive flag is up to the system / One UI.
 */
class SharePayload(val text: String, val subject: String?, val sensitive: Boolean = false) {
    override fun toString(): String = "SharePayload(<redacted>)"
}

/**
 * What the `ACTION_SEND` intent of a [SharePayload] carries, decided here without Android types so that it can be tested.
 * [sensitiveClip]: also attach the text as a clip flagged `EXTRA_IS_SENSITIVE`. [previewTitle]: `EXTRA_TITLE`, a neutral heading
 * for the sheet's preview; null for a normal item. It does not take the text out of the preview (that is `EXTRA_TEXT`).
 * [subject]: `EXTRA_SUBJECT`; always null for a hidden item, whose title must not leave the app.
 */
class ShareIntentPlan private constructor(
    val text: String,
    val subject: String?,
    val previewTitle: String?,
    val sensitiveClip: Boolean,
) {
    override fun toString(): String = "ShareIntentPlan(<redacted>)"

    companion object {
        fun of(payload: SharePayload, hiddenPreviewTitle: String): ShareIntentPlan = ShareIntentPlan(
            text = payload.text,
            subject = if (payload.sensitive) null else payload.subject,
            previewTitle = if (payload.sensitive) hiddenPreviewTitle else null,
            sensitiveClip = payload.sensitive,
        )
    }
}

/** Result of [ItemSharing.prepare]. */
sealed interface ShareOutcome {
    /** The share sheet may be opened with [payload]. */
    class Ready(val payload: SharePayload) : ShareOutcome

    /** The item has no text or link to share (unreadable item, or files only). */
    data object NothingToShare : ShareOutcome

    /** A hidden item and no screen lock or biometrics is set up: it stays locked. */
    data object BiometricUnavailable : ShareOutcome

    /** A hidden item and the user cancelled or failed the authentication: nothing is shared. */
    data object NotAuthenticated : ShareOutcome
}

/**
 * Decides what "Share" does, in one place that does not depend on Android types.
 *
 * Rule (product decision): a normal item is shared directly; a hidden ("Masqué") item can be shared too, but its content is
 * NEVER read into a payload before [authenticate] returned [AuthOutcome.Success]. The payload is built after the
 * authentication, handed to the caller and not kept anywhere here.
 */
object ItemSharing {

    /** True if the "Share" action should be offered for [item] (does not read the content of a hidden item into a payload). */
    fun canShare(item: TomiItem): Boolean = item.isReadable && hasShareableContent(item)

    suspend fun prepare(item: TomiItem, authenticate: suspend () -> AuthOutcome): ShareOutcome {
        if (!canShare(item)) return ShareOutcome.NothingToShare
        if (item.secret) {
            when (authenticate()) {
                AuthOutcome.Success -> Unit
                AuthOutcome.Unavailable -> return ShareOutcome.BiometricUnavailable
                AuthOutcome.Cancelled, AuthOutcome.Failed -> return ShareOutcome.NotAuthenticated
            }
        }
        val payload = payloadFor(item) ?: return ShareOutcome.NothingToShare
        return ShareOutcome.Ready(payload)
    }

    /**
     * Text of the item as plain text (the displayed text, then the link), with the title as subject.
     * Attachments are not shared yet: the core has no file provider to hand out (see `TomiCore` TODO).
     * Callers must have authenticated first when [TomiItem.secret] is true; use [prepare].
     */
    internal fun payloadFor(item: TomiItem): SharePayload? {
        if (!item.isReadable) return null
        val parts = listOfNotNull(item.text?.plain?.takeIf { it.isNotBlank() }, item.url?.takeIf { it.isNotBlank() })
        val title = item.title?.takeIf { it.isNotBlank() }
        val body = parts.joinToString("\n").ifEmpty { title ?: return null }
        return SharePayload(text = body, subject = title.takeIf { parts.isNotEmpty() }, sensitive = item.secret)
    }

    private fun hasShareableContent(item: TomiItem): Boolean =
        !item.text?.plain.isNullOrBlank() || !item.url.isNullOrBlank() || !item.title.isNullOrBlank()
}
