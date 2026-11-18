package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.findBySlot
import eu.shaffe.tomi.security.AuthOutcome

/** Result of an image action of the viewer (see [ImageActions]). */
sealed interface ImageActionResult {
    /** Done: the viewer may show the image / the file was written / the share sheet was opened. */
    data object Done : ImageActionResult

    /** The image has no readable file (unknown or removed slot, not an image, file not available, no app to receive it). */
    data object Unavailable : ImageActionResult

    /** The file was found but writing it failed. */
    data object Failed : ImageActionResult

    /** A hidden item and no screen lock or biometrics is set up: it stays locked. */
    data object BiometricUnavailable : ImageActionResult

    /** A hidden item and the user cancelled or failed the authentication: nothing was decrypted, written or shared. */
    data object NotAuthenticated : ImageActionResult
}

/**
 * Where the image of an item may go, in one place that does not depend on Android types.
 *
 * Rule (product decision, same as the text share of a hidden item, see [ItemSharing]): an image of a hidden ("Masqué") item is
 * only displayed, written to the gallery or handed to another app after [AuthOutcome.Success] of the SAME biometric gate (same
 * 2-minute window) as "Afficher". The attachment is decrypted into the core cache on demand ([resolveUri]); for a hidden
 * item that call is made only after the authentication, so a refused user triggers no decryption at all.
 *
 * Nothing is kept here: the URI is handed to [save] / [share] and dropped. The only copies of a hidden image are the
 * decrypted file in the core cache (purged by the core) and, on explicit request, the gallery entry or the stream given to
 * the receiving app.
 */
class ImageActions(
    /** `TomiCore.attachmentUri` by SLOT (null when unavailable). Must not throw: map errors to null. */
    private val resolveUri: suspend (itemId: String, slot: Int) -> String?,
) {
    /**
     * Authentication to open the viewer on [item]; a normal item is never asked. The viewer renders nothing for a hidden
     * item until this returned [ImageActionResult.Done] (see `ViewerGate`), and only then asks for the files of its pages.
     */
    suspend fun authorizeView(item: TomiItem, authenticate: suspend () -> AuthOutcome): ImageActionResult =
        gate(item, authenticate)

    /**
     * File (decrypted into the core cache on demand) of one page of the viewer or of one code file, for DISPLAY. This is the
     * rule enforced where the file is produced, not only by the screens: for a hidden item the file is requested from the core
     * only while the authorization of [authorizeView] is still valid ([authorizationValid]: the 2-minute window of the biometric
     * gate is open), otherwise null and nothing is decrypted. A normal item needs no authorization. An unknown ([item] null) or
     * unreadable item gives null: unknown means closed.
     */
    suspend fun fileForView(item: TomiItem?, slot: Int, authorizationValid: () -> Boolean): String? {
        if (item == null || !item.isReadable) return null
        if (item.attachments.findBySlot(slot) == null) return null
        if (item.secret && !authorizationValid()) return null
        return resolveUri(item.id, slot)
    }

    /** "Save to gallery": [write] receives the URI of the decrypted file and returns true when the gallery entry was written. */
    suspend fun save(
        item: TomiItem,
        slot: Int,
        authenticate: suspend () -> AuthOutcome,
        write: suspend (uri: String, attachment: Attachment) -> Boolean,
    ): ImageActionResult = guarded(item, slot, authenticate) { uri, attachment ->
        if (write(uri, attachment)) ImageActionResult.Done else ImageActionResult.Failed
    }

    /** "Share": [launch] opens the system share sheet with the URI and returns false when no app can receive it. */
    suspend fun share(
        item: TomiItem,
        slot: Int,
        authenticate: suspend () -> AuthOutcome,
        launch: (uri: String, attachment: Attachment) -> Boolean,
    ): ImageActionResult = guarded(item, slot, authenticate) { uri, attachment ->
        if (launch(uri, attachment)) ImageActionResult.Done else ImageActionResult.Unavailable
    }

    private suspend fun guarded(
        item: TomiItem,
        slot: Int,
        authenticate: suspend () -> AuthOutcome,
        action: suspend (uri: String, attachment: Attachment) -> ImageActionResult,
    ): ImageActionResult {
        val attachment = imageAt(item, slot) ?: return ImageActionResult.Unavailable
        val gated = gate(item, authenticate)
        if (gated != ImageActionResult.Done) return gated
        val uri = resolveUri(item.id, slot) ?: return ImageActionResult.Unavailable
        return action(uri, attachment)
    }

    private suspend fun gate(item: TomiItem, authenticate: suspend () -> AuthOutcome): ImageActionResult {
        if (!item.isReadable) return ImageActionResult.Unavailable
        if (!item.secret) return ImageActionResult.Done
        return when (authenticate()) {
            AuthOutcome.Success -> ImageActionResult.Done
            AuthOutcome.Unavailable -> ImageActionResult.BiometricUnavailable
            AuthOutcome.Cancelled, AuthOutcome.Failed -> ImageActionResult.NotAuthenticated
        }
    }

    private fun imageAt(item: TomiItem, slot: Int): Attachment? =
        item.attachments.findBySlot(slot)?.takeIf { it.isImage }
}
