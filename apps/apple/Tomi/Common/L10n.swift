import Foundation

/// Texts of the app: the keys of `Localizable.xcstrings`, generated from the Android `strings.xml`
/// (same keys, same French texts). TomiKit returns keys and arguments; this resolves them.
enum L10n {
    /// The text of `key`, formatted with `args` (`%1$@`, `%1$lld`...).
    static func text(_ key: String, _ args: CVarArg...) -> String {
        let format = NSLocalizedString(key, comment: "")
        return args.isEmpty ? format : String(format: format, locale: Locale(identifier: "fr_FR"), arguments: args)
    }

    /// A plural text: `count` selects the variation and is the first argument.
    static func plural(_ key: String, _ count: Int, _ more: CVarArg...) -> String {
        let format = NSLocalizedString(key, comment: "")
        return String(format: format, locale: Locale(identifier: "fr_FR"), arguments: [count] + more)
    }
}
