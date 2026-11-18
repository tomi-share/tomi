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
 * Wraps BiometricPrompt. Normal checks (reveal / copy a secret) accept biometrics or the screen lock and are
 * remembered for two minutes. Destructive checks (revoke a device) use [strongOnly]: strong biometrics only,
 * never remembered, always prompted.
 */
class BiometricGate(private val window: AuthWindow) {

    fun isWindowOpen(): Boolean = window.isValid()

    fun invalidateWindow() = window.invalidate()

    suspend fun authenticate(
        activity: FragmentActivity,
        title: String,
        subtitle: String?,
        cancelLabel: String,
        strongOnly: Boolean = false,
    ): AuthOutcome {
        if (!strongOnly && window.isValid()) return AuthOutcome.Success

        val authenticators = if (strongOnly) BIOMETRIC_STRONG else BIOMETRIC_STRONG or DEVICE_CREDENTIAL
        if (BiometricManager.from(activity).canAuthenticate(authenticators) != BiometricManager.BIOMETRIC_SUCCESS) {
            return AuthOutcome.Unavailable
        }
        // Once the activity's state is saved, BiometricPrompt only logs and shows nothing: the callback would never come and the
        // caller would wait forever. Count it as cancelled so that the screen never stays locked on a prompt nobody sees.
        if (activity.supportFragmentManager.isStateSaved) return AuthOutcome.Cancelled

        return suspendCancellableCoroutine { continuation ->
            val callback = object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    if (!strongOnly) window.markAuthenticated()
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
                .apply { if (strongOnly) setNegativeButtonText(cancelLabel) }
                .setConfirmationRequired(false)
                .build()
            prompt.authenticate(info)
            continuation.invokeOnCancellation { prompt.cancelAuthentication() }
        }
    }
}
