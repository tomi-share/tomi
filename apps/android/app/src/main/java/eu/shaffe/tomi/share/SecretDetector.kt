package eu.shaffe.tomi.share

/**
 * Heuristics deciding whether a piece of text "looks like a secret" (password, token, key).
 * The result is only a suggestion: the user can always change the "secret" switch.
 */
object SecretDetector {

    private val PASSWORD_MANAGER_PACKAGES = setOf(
        "com.dashlane",
        "com.x8bit.bitwarden",
        "com.bitwarden.authenticator",
        "com.agilebits.onepassword",
        "com.onepassword.android",
        "org.keepassdroid",
        "keepass2android.keepass2android",
        "com.lastpass.lpandroid",
        "proton.android.pass",
        "me.proton.android.pass",
    )

    private val KNOWN_TOKEN_PREFIXES = listOf("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza", "eyJ")

    private val EMAIL = Regex("^[^@\\s]+@[^@\\s]+\\.[A-Za-z]{2,}$")

    /** True when the share comes from a known password manager. */
    fun isPasswordManager(sourcePackage: String?): Boolean =
        sourcePackage != null && sourcePackage in PASSWORD_MANAGER_PACKAGES

    /**
     * Single token, no whitespace, mixing character classes (or long and hex-like), or a recognisable
     * private-key block / well-known token prefix.
     */
    fun looksLikeSecret(text: String?): Boolean {
        val value = text?.trim() ?: return false
        if (value.isEmpty()) return false
        if (value.contains("-----BEGIN") && value.contains("PRIVATE KEY")) return true
        if (value.any { it.isWhitespace() }) return false
        if (value.length < MIN_LENGTH || value.length > MAX_LENGTH) return false
        if (value.contains("://") || EMAIL.matches(value)) return false
        if (KNOWN_TOKEN_PREFIXES.any { value.startsWith(it) } && value.length >= 16) return true

        var lower = false
        var upper = false
        var digit = false
        var symbol = false
        for (ch in value) {
            when {
                ch.isLowerCase() -> lower = true
                ch.isUpperCase() -> upper = true
                ch.isDigit() -> digit = true
                else -> symbol = true
            }
        }
        val classes = listOf(lower, upper, digit, symbol).count { it }
        return classes >= 3 || (classes == 2 && digit && value.length >= LONG_TOKEN_LENGTH)
    }

    private const val MIN_LENGTH = 8
    private const val MAX_LENGTH = 512
    private const val LONG_TOKEN_LENGTH = 20
}
