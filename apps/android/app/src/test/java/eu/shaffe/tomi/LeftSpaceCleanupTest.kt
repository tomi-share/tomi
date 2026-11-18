package eu.shaffe.tomi

import eu.shaffe.tomi.security.SpaceClipboard
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the app forgets outside the core once the device is no longer in a space */
class LeftSpaceCleanupTest {
    /** The clipboard: [focused] says whether Android lets the app tell whose clip it is; [cleared] counts the clips of Tomi removed. */
    private class Clipboard(var focused: Boolean, var holdsATomiClip: Boolean = true) : SpaceClipboard {
        var cleared = 0

        override fun forgetSpace(): Boolean {
            if (!focused) return false
            if (holdsATomiClip) {
                holdsATomiClip = false
                cleared++
            }
            return true
        }
    }

    private class Setup(val prefs: UserPrefs, val clipboard: Clipboard, val done: MutableList<String>, val cleanup: LeftSpaceCleanup)

    private fun setup(focused: Boolean): Setup {
        val prefs = UserPrefs(MemoryPrefs()).apply { onboardingDone = true }
        val clipboard = Clipboard(focused)
        val done = mutableListOf<String>()
        val cleanup = LeftSpaceCleanup(
            prefs = prefs,
            clipboard = clipboard,
            cancelNotifications = { done += "notifications" },
            closeBiometricWindow = { done += "biometric window" },
            deleteFiles = { done += "files" },
            cancelSends = { done += "sends" },
        )
        return Setup(prefs, clipboard, done, cleanup)
    }

    @Test
    fun everythingTheAppKeptOfTheSpaceGoesAndTheHelpOfTheFirstStartComesBack() {
        val s = setup(focused = true)

        s.cleanup.run()

        assertEquals(listOf("notifications", "biometric window", "sends", "files"), s.done)
        assertFalse("the next space starts with the help again", s.prefs.onboardingDone)
        assertEquals("the clip of Tomi is cleared at once", 1, s.clipboard.cleared)
        assertFalse(s.prefs.clipboardClearPending)
    }

    @Test
    fun withoutFocusTheClipboardIsClearedAtTheNextFocus() {
        val s = setup(focused = false)

        s.cleanup.run()
        assertEquals(0, s.clipboard.cleared)
        assertTrue("remembered across a restart", s.prefs.clipboardClearPending)

        s.clipboard.focused = true
        s.cleanup.onWindowFocused()
        assertEquals(1, s.clipboard.cleared)
        assertFalse(s.prefs.clipboardClearPending)

        // Nothing pending any more: a later focus does not touch the clipboard.
        s.clipboard.holdsATomiClip = true
        s.cleanup.onWindowFocused()
        assertEquals(1, s.clipboard.cleared)
    }

    @Test
    fun itCanRunAgainWithoutHarm() {
        val s = setup(focused = true)
        s.cleanup.run()
        s.cleanup.run()
        assertFalse(s.prefs.onboardingDone)
        assertEquals(1, s.clipboard.cleared)
    }

    @Test
    fun aStepThatThrowsSkipsNeitherTheNextOnesNorCrashesTheApp() {
        val prefs = UserPrefs(MemoryPrefs()).apply { onboardingDone = true }
        val done = mutableListOf<String>()
        val cleanup = LeftSpaceCleanup(
            prefs = prefs,
            clipboard = SpaceClipboard { throw IllegalStateException("no clipboard service") },
            cancelNotifications = { throw IllegalStateException("no notification manager") },
            closeBiometricWindow = { done += "biometric window" },
            deleteFiles = { done += "files" },
            cancelSends = { throw IllegalStateException("WorkManager not initialised") },
        )

        cleanup.run()

        assertEquals(listOf("biometric window", "files"), done)
        assertFalse("the help of the first start still comes back", prefs.onboardingDone)
        assertTrue("the clip could not be checked: it is checked at the next focus", prefs.clipboardClearPending)
        cleanup.onWindowFocused()
    }
}
