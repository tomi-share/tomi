package eu.shaffe.tomi

import eu.shaffe.tomi.security.SpaceClipboard

/**
 * What the app keeps OUTSIDE the core, forgotten once this device is no longer in a space (spec/api-v1.md 7.9.6).
 * The core erased its own data; this is the rest. Idempotent: it runs on every
 * "not paired" state, the first one at start-up included, so that a process that died between the erasure and the cleanup still leaves
 * nothing of the old space behind.
 *
 * Kept: the display preferences (render mode, code wrap), the Keystore wrapping key, the notification channels. The screens restart on the
 * welcome screen through a new task (TomiRoot, MainActivity).
 */
internal class LeftSpaceCleanup(
    private val prefs: UserPrefs,
    private val clipboard: SpaceClipboard,
    private val cancelNotifications: () -> Unit,
    private val closeBiometricWindow: () -> Unit,
    /** The staged copies of outgoing files and the code colouring cache. */
    private val deleteFiles: () -> Unit,
    /** The queued send jobs (WorkManager). */
    private val cancelSends: () -> Unit,
) {
    /**
     * Blocking (files, WorkManager): call it off the main thread. Each step stands alone: one that throws neither skips the next ones nor
     * brings the app down at every start (this runs at every start while the device is not paired). Nothing is logged: the paths and the
     * errors of a left space are not worth a trace.
     */
    fun run() {
        step(cancelNotifications)
        step(closeBiometricWindow)
        // A clip Tomi put there goes at once; without window focus Android does not tell whose clip it is: checked at the next focus.
        val clipForgotten = try {
            clipboard.forgetSpace()
        } catch (_: Exception) {
            false
        }
        step { prefs.clipboardClearPending = !clipForgotten }
        // The next space starts with the help of the first start again.
        step { prefs.onboardingDone = false }
        step(cancelSends)
        step(deleteFiles)
    }

    /** A window of the app got focus: a clip that could not be checked when the device left is checked now. */
    fun onWindowFocused() {
        step { if (prefs.clipboardClearPending && clipboard.forgetSpace()) prefs.clipboardClearPending = false }
    }

    private fun step(action: () -> Unit) {
        try {
            action()
        } catch (_: Exception) {
            // Intentionally silent: see run().
        }
    }
}
