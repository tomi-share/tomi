package eu.shaffe.tomi.security

/**
 * Remembers for [windowMs] that the user just passed a biometric check on this device
 * (the validation is valid for 2 minutes).
 */
class AuthWindow(
    private val nowMs: () -> Long,
    private val windowMs: Long = DEFAULT_WINDOW_MS,
) {
    private var validUntilMs: Long = Long.MIN_VALUE

    @Synchronized
    fun markAuthenticated() {
        validUntilMs = nowMs() + windowMs
    }

    @Synchronized
    fun isValid(): Boolean = nowMs() < validUntilMs

    @Synchronized
    fun invalidate() {
        validUntilMs = Long.MIN_VALUE
    }

    companion object {
        const val DEFAULT_WINDOW_MS = 2 * 60 * 1000L
    }
}

/**
 * Decides whether the clipboard must be cleared after a secret was copied. We never read the clip content: we only
 * compare the clip timestamp (`primaryClipDescription.timestamp`) with the one recorded when we put the secret there.
 *
 * KNOWN LIMIT, deliberate, and NOT verified on a device: since Android 10 an app in the background cannot read the
 * clipboard, and `primaryClipDescription` is then expected to be null. The 30 s job runs in the background, so in
 * practice [shouldClear] sees `currentTimestamp == null` and clears unconditionally, even if the user copied
 * something else in the meantime. We accept that: for a secret, wiping is safer than leaving it. The foreground
 * check ([onForeground]) is the more precise complement: when the app is back on screen the timestamp is readable,
 * so a clip that is still ours is cleared, and one that is not is left alone.
 */
object ClipboardClearPolicy {
    const val CLEAR_DELAY_MS = 30_000L

    /**
     * Background job (30 s after the copy).
     * @param expectedTimestamp timestamp of the clip we set (0 if it could not be read back)
     * @param currentTimestamp timestamp of the current primary clip, null if there is none or it is not readable
     */
    fun shouldClear(expectedTimestamp: Long, currentTimestamp: Long?): Boolean = when {
        // Unknown on either side: clearing is the safe choice for a secret.
        expectedTimestamp == 0L || currentTimestamp == null -> true
        else -> expectedTimestamp == currentTimestamp
    }

    enum class ForegroundAction {
        /** The 30 s are not over: the job will do it. */
        Wait,

        /** The clip is still ours and overdue: clear it. */
        Clear,

        /** The clip is not provably ours any more: stop tracking, leave it alone. */
        Forget,

        /** The clip cannot be read yet (no focus, or empty): keep tracking, try again at the next foreground. */
        Retry,
    }

    /**
     * Foreground check, when the app window gets focus. Unlike [shouldClear] it never clears a clip it cannot
     * prove to be ours.
     */
    fun onForeground(expectedTimestamp: Long, currentTimestamp: Long?, nowMs: Long, deadlineMs: Long): ForegroundAction = when {
        nowMs < deadlineMs -> ForegroundAction.Wait
        currentTimestamp == null -> ForegroundAction.Retry
        expectedTimestamp != 0L && currentTimestamp == expectedTimestamp -> ForegroundAction.Clear
        else -> ForegroundAction.Forget
    }
}
