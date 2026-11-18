package eu.shaffe.tomi.ui.send

import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType

/** What the compose screen does differently while the item being written is hidden ("Masqué"). Pure decisions, tested without a device. */
object SecretCompose {

    /**
     * FLAG_SECURE (no screenshot, blank in the recents) as soon as something of a hidden item is on screen: its text, but also its
     * title (hidden too, encrypted, out of the feed and of the search) and the names of its attached files.
     */
    fun needsSecureWindow(secret: Boolean, text: String, title: String, hasAttachments: Boolean): Boolean =
        secret && (text.isNotEmpty() || title.isNotEmpty() || hasAttachments)

    /** Whether the system text toolbar of the compose fields may offer "Copy" and "Cut" (a hidden item must not go to the ordinary clipboard). */
    fun allowsClipboardActions(secret: Boolean): Boolean = !secret

    /**
     * Keyboard of a field whose content must not be learned: password keyboard, no auto-correct, no capital letter imposed, so the IME
     * keeps no suggestion or dictionary entry from it. Accented letters stay available (long press), as in any password field.
     * A field of a normal item keeps the default keyboard.
     */
    fun keyboardOptions(secret: Boolean): KeyboardOptions =
        if (secret) {
            KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Password,
            )
        } else {
            KeyboardOptions.Default
        }
}
