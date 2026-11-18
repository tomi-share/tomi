import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/share/SecretDetector.kt.

/// Heuristics deciding whether a piece of text "looks like a secret" (password, token, key).
/// The result is only a suggestion: the user can always change the "secret" switch.
public enum SecretDetector {
    /// Android package names of the known password managers. On iOS the share extension has no
    /// referrer: `sourcePackage` is whatever identifier the app can obtain (bundle id), often nil.
    private static let passwordManagerPackages: Set<String> = [
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
    ]

    private static let knownTokenPrefixes = ["sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza", "eyJ"]

    private static let email = try! NSRegularExpression(pattern: "\\A[^@\\s]+@[^@\\s]+\\.[A-Za-z]{2,}\\z")

    private static let minLength = 8
    private static let maxLength = 512
    private static let longTokenLength = 20

    /// True when the share comes from a known password manager.
    public static func isPasswordManager(_ sourcePackage: String?) -> Bool {
        guard let sourcePackage else { return false }
        return passwordManagerPackages.contains(sourcePackage)
    }

    /// Single token, no whitespace, mixing character classes (or long and hex-like), or a recognisable
    /// private-key block / well-known token prefix.
    public static func looksLikeSecret(_ text: String?) -> Bool {
        guard let value = text?.trimmingCharacters(in: .whitespacesAndNewlines) else { return false }
        if value.isEmpty { return false }
        if value.contains("-----BEGIN") && value.contains("PRIVATE KEY") { return true }
        if value.contains(where: \.isWhitespace) { return false }
        // Lengths and classes follow Kotlin, which works on UTF-16 units (Java chars): never weaker than Android.
        let length = value.utf16.count
        if length < minLength || length > maxLength { return false }
        if value.contains("://") || isEmail(value) { return false }
        if knownTokenPrefixes.contains(where: { value.hasPrefix($0) }) && length >= 16 { return true }

        var lower = false
        var upper = false
        var digit = false
        var symbol = false
        for scalar in value.unicodeScalars {
            // A character outside the BMP is two Java chars (surrogates), neither a letter nor a digit there.
            if scalar.value > 0xFFFF {
                symbol = true
            } else if scalar.properties.isLowercase {
                lower = true
            } else if scalar.properties.isUppercase {
                upper = true
            } else if scalar.properties.generalCategory == .decimalNumber {
                digit = true
            } else {
                symbol = true
            }
        }
        let classes = [lower, upper, digit, symbol].filter { $0 }.count
        return classes >= 3 || (classes == 2 && digit && length >= longTokenLength)
    }

    private static func isEmail(_ value: String) -> Bool {
        email.firstMatch(in: value, range: NSRange(location: 0, length: (value as NSString).length)) != nil
    }
}
