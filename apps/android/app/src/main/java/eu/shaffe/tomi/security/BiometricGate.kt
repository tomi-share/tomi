package eu.shaffe.tomi.security

import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

enum class AuthOutcome {
    Success,
    Cancelled,

    /** No suitable biometric/credential is enrolled: the protected action stays locked. */
    Unavailable,
    Failed,
}

/**
 * What a check accepts and whether it may be remembered. Pure, so that it can be tested without Android.
 *
 * Both modes accept strong biometrics OR the screen lock (PIN, pattern, password): a phone with only a PIN must still be able to
 * revoke a stolen device. They differ only in memory: a [fresh] check (revoke, delete the space, erase, add a device) ignores
 * the 2-minute window and never opens it, so it always prompts.
 */
internal object AuthPolicy {
    const val AUTHENTICATORS = BIOMETRIC_STRONG or DEVICE_CREDENTIAL

    /** The remembered window answers the check only for a normal (non-fresh) one. */
    fun answeredByWindow(fresh: Boolean, windowOpen: Boolean): Boolean = !fresh && windowOpen

    /** A success opens the 2-minute window only for a normal check: a sensitive success must not unlock the secrets. */
    fun opensWindow(fresh: Boolean): Boolean = !fresh
}

/**
 * Wraps BiometricPrompt. Normal checks (reveal / copy a secret) accept biometrics or the screen lock and are
 * remembered for two minutes. Sensitive checks (revoke a device, delete the space, erase, add a device) use [fresh]:
 * same authenticators, never remembered, always prompted.
 */
class BiometricGate(private val window: AuthWindow) {

    fun isWindowOpen(): Boolean = window.isValid()

    fun invalidateWindow() = window.invalidate()

    suspend fun authenticate(
        activity: FragmentActivity,
        title: String,
        subtitle: String?,
        cancelLabel: String,
        fresh: Boolean = false,
    ): AuthOutcome {
        if (AuthPolicy.answeredByWindow(fresh, window.isValid())) return AuthOutcome.Success

        val authenticators = AuthPolicy.AUTHENTICATORS
        if (BiometricManager.from(activity).canAuthenticate(authenticators) != BiometricManager.BIOMETRIC_SUCCESS) {
            return AuthOutcome.Unavailable
        }
        // Once the activity's state is saved, BiometricPrompt only logs and shows nothing: the callback would never come and the
        // caller would wait forever. Count it as cancelled so that the screen never stays locked on a prompt nobody sees.
        if (activity.supportFragmentManager.isStateSaved) return AuthOutcome.Cancelled

        return suspendCancellableCoroutine { continuation ->
            val callback = object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    if (AuthPolicy.opensWindow(fresh)) window.markAuthenticated()
                    if (continuation.isActive) continuation.resume(AuthOutcome.Success)
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    val cancelled = errorCode == BiometricPrompt.ERROR_USER_CANCELED ||
                        errorCode == BiometricPrompt.ERROR_NEGATIVE_BUTTON ||
                        errorCode == BiometricPrompt.ERROR_CANCELED
                    if (continuation.isActive) continuation.resume(if (cancelled) AuthOutcome.Cancelled else AuthOutcome.Failed)
                }
            }
            val prompt = BiometricPrompt(activity, ContextCompat.getMainExecutor(activity), callback)
            val info = BiometricPrompt.PromptInfo.Builder()
                .setTitle(title)
                .apply { if (subtitle != null) setSubtitle(subtitle) }
                .setAllowedAuthenticators(authenticators)
                // No negative button: it is forbidden together with DEVICE_CREDENTIAL (the system provides "cancel").
                // A passive face unlock must not authorize a sensitive action just by pointing the phone at the owner.
                .setConfirmationRequired(fresh)
                .build()
            prompt.authenticate(info)
            continuation.invokeOnCancellation { prompt.cancelAuthentication() }
        }
    }
}
