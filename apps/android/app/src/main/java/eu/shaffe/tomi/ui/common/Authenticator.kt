package eu.shaffe.tomi.ui.common

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import eu.shaffe.tomi.TomiApplication
import eu.shaffe.tomi.R
import eu.shaffe.tomi.security.AuthOutcome
import androidx.fragment.app.FragmentActivity

/** Binds [eu.shaffe.tomi.security.BiometricGate] to the current activity and the French prompt texts. */
class Authenticator(private val activity: FragmentActivity) {
    private val gate = (activity.application as TomiApplication).container.biometricGate

    /** Reveal / copy a secret: biometrics or screen lock, remembered for two minutes. */
    suspend fun forSecret(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_secret),
        subtitle = activity.getString(R.string.biometric_subtitle_secret),
        cancelLabel = activity.getString(R.string.action_cancel),
    )

    /** Share a hidden item to another app: the same gate (and 2-minute window) as [forSecret], with its own prompt text. */
    suspend fun forShare(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_share),
        subtitle = activity.getString(R.string.biometric_subtitle_share),
        cancelLabel = activity.getString(R.string.action_cancel),
    )

    /** Open the image of a hidden item in the viewer: same gate (and 2-minute window) as [forSecret], with its own prompt text. */
    suspend fun forImageView(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_image),
        subtitle = activity.getString(R.string.biometric_subtitle_image),
        cancelLabel = activity.getString(R.string.action_cancel),
    )

    /** Open the code file of a hidden item in the code screen: same gate (and 2-minute window) as [forSecret], with its own prompt text. */
    suspend fun forFileView(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_file),
        subtitle = activity.getString(R.string.biometric_subtitle_file),
        cancelLabel = activity.getString(R.string.action_cancel),
    )

    /** Write the image of a hidden item to the gallery: same gate (and 2-minute window) as [forSecret], with its own prompt text. */
    suspend fun forImageSave(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_save_image),
        subtitle = activity.getString(R.string.biometric_subtitle_save_image),
        cancelLabel = activity.getString(R.string.action_cancel),
    )

    /** Destructive action (revoke a device): strong biometrics or the screen lock, never remembered, always asked. */
    suspend fun forRevocation(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_revoke),
        subtitle = activity.getString(R.string.biometric_subtitle_revoke),
        cancelLabel = activity.getString(R.string.action_cancel),
        fresh = true,
    )

    /**
     * "Ajouter un appareil" (opening the invitation, and the final "codes identiques"): the new device receives the whole history,
     * secrets and files included. Same gate as [forRevocation]: the screen lock or strong biometrics, never remembered, always asked.
     */
    suspend fun forPairing(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_pair),
        subtitle = activity.getString(R.string.biometric_subtitle_pair),
        cancelLabel = activity.getString(R.string.action_cancel),
        fresh = true,
    )

    /** "Supprimer l'espace" (the only device): same gate as [forRevocation] (fresh, never remembered). */
    suspend fun forSpaceDeletion(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_delete_space),
        subtitle = activity.getString(R.string.biometric_subtitle_delete_space),
        cancelLabel = activity.getString(R.string.action_cancel),
        fresh = true,
    )

    /** "Effacer quand même" (erase without a proof of the relay): same gate as [forRevocation] (fresh, never remembered). */
    suspend fun forErasure(): AuthOutcome = gate.authenticate(
        activity = activity,
        title = activity.getString(R.string.biometric_title_erase),
        subtitle = activity.getString(R.string.biometric_subtitle_erase),
        cancelLabel = activity.getString(R.string.action_cancel),
        fresh = true,
    )
}

@Composable
fun rememberAuthenticator(): Authenticator? {
    val context = LocalContext.current
    return remember(context) { context.findFragmentActivity()?.let { Authenticator(it) } }
}
