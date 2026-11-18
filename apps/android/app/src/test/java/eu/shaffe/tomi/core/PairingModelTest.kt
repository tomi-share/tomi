package eu.shaffe.tomi.core

import eu.shaffe.tomi.ui.pairing.InviteUiState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The secrets of a pairing print as redacted wherever they end up: a log line, a crash report, a failed assertion, a `"$state"`
 * (spec/pairing-v1.md 12: the verification code and the pairing code are never logged).
 */
class PairingModelTest {
    private val sas = "482 913"
    private val peer = PairingPeer("PC-Bureau", DevicePlatform.Windows)

    private fun assertNoSecret(printed: String) {
        assertFalse(printed, printed.contains("482"))
        assertFalse(printed, printed.contains("913"))
        assertFalse(printed, printed.contains("crayon"))
        assertFalse(printed, printed.contains("renard"))
        assertFalse(printed, printed.contains("h7k2"))
        assertTrue(printed, printed.contains("redacted"))
    }

    @Test
    fun theVerificationCodeIsDrawnFromItsDigitsWhateverTheSeparator() {
        assertEquals("482913", VerificationCode(sas).digits)
        assertEquals("482913", VerificationCode("482913").digits)
        assertEquals("482 913", VerificationCode("482913").grouped)
        assertEquals("482 913", VerificationCode(sas).grouped)
    }

    @Test
    fun theVerificationCodePrintsRedacted() {
        val code = VerificationCode(sas)
        assertNoSecret(code.toString())
        assertNoSecret("$code")
        assertNoSecret("code=" + code)
    }

    @Test
    fun theSensitiveTextPrintsRedactedButIsShownByReveal() {
        val text = SensitiveText("4821-crayon-renard")
        assertNoSecret(text.toString())
        assertNoSecret("$text")
        assertEquals("4821-crayon-renard", text.reveal())
        assertTrue(SensitiveText("  ").isBlank)
        assertFalse(text.isBlank)
    }

    @Test
    fun everyModelThatCarriesASecretPrintsRedacted() {
        assertNoSecret(PairingResult.AwaitingVerification(peer, VerificationCode(sas), historyItemCount = 3).toString())
        assertNoSecret(InvitePrompt(peer, VerificationCode(sas)).toString())
        assertNoSecret(InviteUiState.Verify(peer, VerificationCode(sas)).toString())
        assertNoSecret(InviteUiState.ShowCode(SensitiveText("4821-crayon-renard"), SensitiveText("https://relay.example.org/l/pair#4821-crayon-renard"), 0L).toString())
    }

    @Test
    fun aPairingExceptionNamesTheReasonAndNothingElse() {
        val e = PairingException(PairingFailure.Taken)
        assertEquals("pairing failed: Taken", e.message)
    }
}
