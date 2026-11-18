package eu.shaffe.tomi.ui.send

import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the compose screen does while the item being written is hidden ("Masqué"). */
class SecretComposeTest {

    @Test
    fun theWindowIsSecuredForAHiddenItemAsSoonAsAnythingOfItIsOnScreen() {
        assertTrue(SecretCompose.needsSecureWindow(secret = true, text = "mot de passe", title = "", hasAttachments = false))
        // Title alone: a screenshot would show it, although it is hidden and encrypted.
        assertTrue(SecretCompose.needsSecureWindow(secret = true, text = "", title = "Wi-Fi", hasAttachments = false))
        // File names alone.
        assertTrue(SecretCompose.needsSecureWindow(secret = true, text = "", title = "", hasAttachments = true))
        assertTrue(SecretCompose.needsSecureWindow(secret = true, text = "a", title = "b", hasAttachments = true))
    }

    @Test
    fun theWindowIsNotSecuredWhenNothingIsOnScreenOrTheItemIsNotHidden() {
        assertFalse(SecretCompose.needsSecureWindow(secret = true, text = "", title = "", hasAttachments = false))
        assertFalse(SecretCompose.needsSecureWindow(secret = false, text = "bonjour", title = "Doc", hasAttachments = true))
        assertFalse(SecretCompose.needsSecureWindow(secret = false, text = "", title = "", hasAttachments = false))
    }

    @Test
    fun aHiddenItemGetsALearningFreeKeyboardAndANormalOneKeepsTheDefault() {
        val hidden = SecretCompose.keyboardOptions(secret = true)
        assertEquals(KeyboardType.Password, hidden.keyboardType)
        assertEquals(false, hidden.autoCorrectEnabled)
        assertEquals(KeyboardCapitalization.None, hidden.capitalization)
        assertEquals(androidx.compose.foundation.text.KeyboardOptions.Default, SecretCompose.keyboardOptions(secret = false))
    }
}
