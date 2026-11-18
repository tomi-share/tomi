package eu.shaffe.tomi.share

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SecretDetectorTest {

    @Test
    fun mixedSingleWordIsASecret() {
        assertTrue(SecretDetector.looksLikeSecret("Tr0ub4dor&3"))
        assertTrue(SecretDetector.looksLikeSecret("  correcthorse9Battery!  "))
    }

    @Test
    fun longHexTokenIsASecret() {
        assertTrue(SecretDetector.looksLikeSecret("3f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c"))
    }

    @Test
    fun knownTokenPrefixesAndPrivateKeysAreSecrets() {
        assertTrue(SecretDetector.looksLikeSecret("ghp_abcdefghijklmnopqrstuvwxyz"))
        assertTrue(SecretDetector.looksLikeSecret("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"))
    }

    @Test
    fun ordinaryTextIsNotASecret() {
        assertFalse(SecretDetector.looksLikeSecret("Rendez-vous demain à 14 h"))
        assertFalse(SecretDetector.looksLikeSecret("bonjour"))
        assertFalse(SecretDetector.looksLikeSecret("crayon-lune"))
        assertFalse(SecretDetector.looksLikeSecret(""))
        assertFalse(SecretDetector.looksLikeSecret(null))
    }

    @Test
    fun urlsAndEmailsAreNotSecrets() {
        assertFalse(SecretDetector.looksLikeSecret("https://example.org/A1b2C3d4E5"))
        assertFalse(SecretDetector.looksLikeSecret("jean.dupont42@example.org"))
    }

    @Test
    fun tooShortOrTooLongIsNotASecret() {
        assertFalse(SecretDetector.looksLikeSecret("Ab1!"))
        assertFalse(SecretDetector.looksLikeSecret("A1!".repeat(300)))
    }

    @Test
    fun knownPasswordManagersAreRecognised() {
        assertTrue(SecretDetector.isPasswordManager("com.x8bit.bitwarden"))
        assertFalse(SecretDetector.isPasswordManager("com.android.chrome"))
        assertFalse(SecretDetector.isPasswordManager(null))
    }
}
