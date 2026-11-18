import Foundation

// Port of apps/android/.../share/TextSanitizer.kt.

/// Cleans untrusted text coming from shares and deep links.
public enum TextSanitizer {
    /// Removes control characters (except `\n` and `\t`) and bidirectional override characters,
    /// normalises line endings, and trims the result. Returns nil if nothing is left. `maxLength`
    /// counts UTF-16 units, like Kotlin (a cut never splits a character).
    public static func clean(_ raw: String?, maxLength: Int = .max) -> String? {
        guard let raw else { return nil }
        let normalised = raw.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        var scalars = String.UnicodeScalarView()
        for scalar in normalised.unicodeScalars where !isDropped(scalar) {
            scalars.append(scalar)
        }
        let out = String(scalars).trimmingCharacters(in: .whitespacesAndNewlines)
        if out.isEmpty { return nil }
        guard out.utf16.count > maxLength else { return out }
        var cut = ""
        var units = 0
        for ch in out {
            let n = ch.utf16.count
            if units + n > maxLength { break }
            cut.append(ch)
            units += n
        }
        return cut
    }

    private static func isDropped(_ scalar: Unicode.Scalar) -> Bool {
        if scalar == "\n" || scalar == "\t" { return false }
        let v = scalar.value
        // ISO control characters (Kotlin `Char.isISOControl`).
        if v <= 0x1F || (0x7F...0x9F).contains(v) { return true }
        // Bidi embedding / override / isolate characters can spoof what a link or a title says.
        return (0x202A...0x202E).contains(v) || (0x2066...0x2069).contains(v)
    }
}
