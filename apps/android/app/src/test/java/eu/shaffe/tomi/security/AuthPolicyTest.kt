package eu.shaffe.tomi.security

import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** What a check accepts and what it remembers: sensitive actions must be usable with a PIN alone, and never answered from memory. */
class AuthPolicyTest {
    @Test
    fun everyCheckAcceptsStrongBiometricsOrTheScreenLock() {
        assertEquals(BIOMETRIC_STRONG or DEVICE_CREDENTIAL, AuthPolicy.AUTHENTICATORS)
        assertTrue(AuthPolicy.AUTHENTICATORS and DEVICE_CREDENTIAL != 0)
    }

    @Test
    fun aFreshCheckIsNeverAnsweredByTheOpenWindow() {
        assertFalse(AuthPolicy.answeredByWindow(fresh = true, windowOpen = true))
        assertFalse(AuthPolicy.answeredByWindow(fresh = true, windowOpen = false))
    }

    @Test
    fun aNormalCheckIsAnsweredByTheOpenWindowOnly() {
        assertTrue(AuthPolicy.answeredByWindow(fresh = false, windowOpen = true))
        assertFalse(AuthPolicy.answeredByWindow(fresh = false, windowOpen = false))
    }

    @Test
    fun aFreshSuccessDoesNotOpenTheSecretsWindow() {
        assertFalse(AuthPolicy.opensWindow(fresh = true))
        assertTrue(AuthPolicy.opensWindow(fresh = false))
    }
}
