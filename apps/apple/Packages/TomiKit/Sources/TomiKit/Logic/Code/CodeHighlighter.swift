import Foundation

// Port of apps/windows/src/lib/highlight.ts: a small syntax highlighter for the code screen. No dependency and no
// network: a hand-written tokenizer that knows comments, strings, numbers, keywords and, for markup, tags and
// attributes. It is a colouring aid, not a parser: an unknown construct stays uncoloured, and nothing it gets wrong
// changes the text shown (only its colours).
//
// Offsets are UTF-16 code units of the text given, as in `CodeDocument`.
//
// Differences with the Windows original, all cosmetic: a `#` only starts a shell or YAML comment after a space (`$#`,
// a URL with an anchor), a YAML quote only starts a string where a value can start (`it's`), a Rust `'a` lifetime is
// not a string, TOML keys are coloured, and keys (JSON, YAML, TOML) have their own kind.

/// What a token is, for its colour (the palette is in the app: Android `CodePalette`).
public enum CodeTokenKind: Hashable, Sendable, CaseIterable {
    case comment, string, number, keyword, literal, tag, attribute, property, function
}

/// A coloured piece of the text: [start, end) in UTF-16 code units.
public struct CodeToken: Hashable, Sendable {
    public let start: Int
    public let end: Int
    public let kind: CodeTokenKind

    public init(start: Int, end: Int, kind: CodeTokenKind) {
        self.start = start
        self.end = end
        self.kind = kind
    }
}

/// A coloured piece of one line (a token that crosses lines is cut at each line end).
public struct LineRun: Hashable, Sendable {
    public let startCol: Int
    public let endCol: Int
    public let kind: CodeTokenKind

    public init(startCol: Int, endCol: Int, kind: CodeTokenKind) {
        self.startCol = startCol
        self.endCol = endCol
        self.kind = kind
    }
}

public extension CodeLines {
    /// Tokens of the whole text, cut at line ends and grouped by line (Android `bucketRuns`). A line without colour has
    /// no entry; the runs of a line are in the order of the text.
    static func bucketRuns(_ doc: CodeDocument, tokens: [CodeToken]) -> [Int: [LineRun]] {
        var result = [Int: [LineRun]]()
        for token in tokens {
            forEachLinePiece(doc, start: token.start, end: token.end) { line, a, b in
                result[line, default: []].append(LineRun(startCol: a, endCol: b, kind: token.kind))
            }
        }
        return result
    }
}

/// What the code screen shows of a file: its colours by line, or why it has none (the screen says so).
public enum CodeColours: Hashable, Sendable {
    case coloured([Int: [LineRun]])
    /// No language colours this file (Android: "Pas de coloration pour ce langage").
    case noLanguage
    /// Over 50 KB or 2,000 lines (`CodeLimits.canHighlight`).
    case tooLong

    /// The key of the text shown above the code, nil when the file is coloured.
    public var noticeKey: String? {
        switch self {
        case .coloured: nil
        case .noLanguage: "code_no_colours_language"
        case .tooLong: "code_no_colours_size"
        }
    }

    public var runs: [Int: [LineRun]] {
        if case let .coloured(runs) = self { runs } else { [:] }
    }
}

public enum CodeHighlighter {
    /// The colours of the file `fileName` whose text shown is `doc`, or the reason there are none. The language is
    /// looked at first, as on Android: a long file in a language without colours says "pas de coloration pour ce
    /// langage", not that it is too long.
    public static func colours(fileName: String, doc: CodeDocument) -> CodeColours {
        guard let language = language(forFileName: fileName) else { return .noLanguage }
        guard CodeLimits.canHighlight(text: doc.text, lineCount: doc.lineCount) else { return .tooLong }
        return .coloured(CodeLines.bucketRuns(doc, tokens: tokens(doc.text, language: language)))
    }

    /// The language that colours `fileName`, or nil when there is none (the screen says so). The grammars of Android
    /// (`CodeLanguages.grammarFor`), plus the code extensions Android leaves uncoloured and Windows colours.
    public static func language(forFileName fileName: String) -> String? {
        if let grammar = CodeLanguages.grammarFor(fileName) { return grammars[grammar] == nil ? nil : grammar }
        guard let ext = CodeLanguages.extensionOf(fileName) else { return nil }
        return extraByExtension[ext]
    }

    /// Colours `text` with `language` (a name given by `language(forFileName:)`). Tokens are in order, never overlap
    /// and never go past the text. An unknown language gives no token.
    public static func tokens(_ text: String, language: String) -> [CodeToken] {
        guard let grammar = grammars[language] else { return [] }
        let units = Array(text.utf16)
        return grammar.markdown ? markdownTokens(units) : scan(units, grammar)
    }

    // MARK: Grammars

    private struct Grammar {
        var line: [[UInt16]] = []
        var block: [(open: [UInt16], close: [UInt16])] = []
        var quotes: [UInt16] = []
        /// Quotes whose string may run over several lines (template literals, raw strings).
        var multiline: [UInt16] = []
        var triple: [[UInt16]] = []
        var keywords: Set<String> = []
        var literals: Set<String> = []
        /// Keywords are case-insensitive (SQL).
        var caseInsensitive = false
        /// `#rrggbb` colours and `@rules` (CSS).
        var css = false
        /// A string or a word followed by `:` is a key (JSON, YAML).
        var keys = false
        /// A word followed by `=` is a key (TOML).
        var keyEquals = false
        var markup = false
        /// Markdown: headings, fences and inline code.
        var markdown = false
        /// `'a` is a lifetime, not the start of a string (Rust).
        var lifetimes = false
        /// `#` starts a comment only at the start of a line or after a space (shell, YAML).
        var hashAfterSpace = false
        /// A quote starts a string only where a value can start (YAML: `it's` is plain text).
        var quoteAfterBoundary = false
    }

    private static func u(_ string: String) -> [UInt16] { Array(string.utf16) }
    private static func words(_ string: String) -> Set<String> { Set(string.split(separator: " ").map(String.init)) }

    private static func cLike(
        quotes: String = "\"'", multiline: String = "", keywords: String, literals: String, lifetimes: Bool = false
    ) -> Grammar {
        Grammar(
            line: [u("//")], block: [(u("/*"), u("*/"))], quotes: u(quotes), multiline: u(multiline),
            keywords: words(keywords), literals: words(literals), lifetimes: lifetimes
        )
    }

    private static let javascript = cLike(
        quotes: "\"'`", multiline: "`",
        keywords: "async await break case catch class const continue debugger default delete do else export extends finally for from function if import in instanceof let new of return static super switch this throw try typeof var void while with yield as get set type interface enum implements namespace declare readonly abstract private protected public keyof satisfies",
        literals: "true false null undefined NaN Infinity"
    )

    private static let markup = Grammar(block: [(u("<!--"), u("-->"))], quotes: u("\"'"), markup: true)

    private static let shell = Grammar(
        line: [u("#")], quotes: u("\"'"),
        keywords: words("if then else elif fi for while until do done case esac in function select time return exit export local readonly unset shift source alias echo cd set eval exec trap test"),
        literals: words("true false"), hashAfterSpace: true
    )

    /// By name: the grammar names of `CodeLanguages` (Android), then the ones only this highlighter has.
    private static let grammars: [String: Grammar] = [
        "javascript": javascript,
        "typescript": javascript,
        "tsx": javascript,
        "java": cLike(
            keywords: "abstract assert boolean break byte case catch char class const continue default do double else enum extends final finally float for goto if implements import instanceof int interface long native new package private protected public return short static strictfp super switch synchronized this throw throws transient try var void volatile while record sealed permits",
            literals: "true false null"
        ),
        "c": cLike(
            keywords: "auto break case char const continue default do double else enum extern float for goto if inline int long register restrict return short signed sizeof static struct switch typedef union unsigned void volatile while class namespace template typename public private protected virtual override new delete using bool constexpr",
            literals: "true false NULL nullptr"
        ),
        "go": cLike(
            quotes: "\"'`", multiline: "`",
            keywords: "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var",
            literals: "true false nil iota"
        ),
        "rust": cLike(
            keywords: "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait type unsafe use where while",
            literals: "true false None Some Ok Err", lifetimes: true
        ),
        "php": Grammar(
            line: [u("//"), u("#")], block: [(u("/*"), u("*/"))], quotes: u("\"'"),
            keywords: words("abstract and array as break callable case catch class clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile extends final finally fn for foreach function global if implements include include_once instanceof interface isset list match namespace new or print private protected public readonly require require_once return static switch throw trait try unset use var while xor yield"),
            literals: words("true false null TRUE FALSE NULL")
        ),
        "python": Grammar(
            line: [u("#")], quotes: u("\"'"), triple: [u("\"\"\""), u("'''")],
            keywords: words("and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield match case"),
            literals: words("True False None self")
        ),
        "bash": shell,
        "yaml": Grammar(
            line: [u("#")], quotes: u("\"'"), literals: words("true false null yes no on off True False"),
            keys: true, hashAfterSpace: true, quoteAfterBoundary: true
        ),
        "json": Grammar(quotes: u("\""), literals: words("true false null"), keys: true),
        "css": Grammar(block: [(u("/*"), u("*/"))], quotes: u("\"'"), css: true),
        "html": markup,
        "markdown": Grammar(markdown: true),
        // Not in Android's list (files it shows without colours).
        "swiftlike": cLike(
            keywords: "as break case catch class continue default defer do else enum extension fallthrough fileprivate final for func guard if import in init inout internal is let open operator private protocol public repeat rethrows return self Self static struct subscript super switch throw throws try typealias var where while fun val object when override abstract data sealed suspend lateinit by companion interface package using namespace void int string bool float double long char new delegate event readonly async await partial record base",
            literals: "true false nil null"
        ),
        "ruby": Grammar(
            line: [u("#")], block: [(u("=begin"), u("=end"))], quotes: u("\"'"),
            keywords: words("alias and begin break case class def defined do else elsif end ensure for if in module next not or redo rescue retry return super then undef unless until when while yield require include extend attr_accessor attr_reader attr_writer"),
            literals: words("true false nil self")
        ),
        "sql": Grammar(
            line: [u("--")], block: [(u("/*"), u("*/"))], quotes: u("'\""),
            keywords: words("select from where and or not in is like between join inner left right outer full cross on as group by order having limit offset insert into values update set delete create alter drop table index view database primary key foreign references unique default check constraint union all distinct case when then else end exists asc desc with returning begin commit rollback"),
            literals: words("null true false"), caseInsensitive: true
        ),
        "toml": Grammar(
            line: [u("#")], quotes: u("\"'"), triple: [u("\"\"\""), u("'''")], literals: words("true false"), keyEquals: true
        ),
    ]

    private static let extraByExtension: [String: String] = [
        "kt": "swiftlike", "kts": "swiftlike", "swift": "swiftlike", "cs": "swiftlike", "dart": "swiftlike",
        "scala": "swiftlike", "gradle": "swiftlike",
        "rb": "ruby",
        "cpp": "c", "cc": "c", "cxx": "c", "hpp": "c", "hh": "c",
        "sql": "sql", "toml": "toml",
    ]

    // MARK: Characters

    private static let newline: UInt16 = 0x0A
    private static let backslash: UInt16 = 0x5C
    private static let space: UInt16 = 0x20
    private static let tab: UInt16 = 0x09

    private static func ascii(_ character: Character) -> UInt16 { UInt16(character.asciiValue ?? 0) }
    private static func isDigit(_ c: UInt16) -> Bool { c >= 0x30 && c <= 0x39 }
    private static func isLetter(_ c: UInt16) -> Bool { (c >= 0x41 && c <= 0x5A) || (c >= 0x61 && c <= 0x7A) }
    private static func isHex(_ c: UInt16) -> Bool { isDigit(c) || (c >= 0x41 && c <= 0x46) || (c >= 0x61 && c <= 0x66) }
    /// `\w` of the original: ASCII letters, digits and the underscore.
    private static func isWord(_ c: UInt16) -> Bool { isLetter(c) || isDigit(c) || c == 0x5F }
    private static func isBlank(_ c: UInt16) -> Bool { c == space || c == tab }
    private static func isOne(_ c: UInt16, of set: String) -> Bool { set.utf16.contains(c) }

    private static func starts(_ units: [UInt16], _ marker: [UInt16], at i: Int) -> Bool {
        guard !marker.isEmpty, i + marker.count <= units.count else { return false }
        for k in 0 ..< marker.count where units[i + k] != marker[k] { return false }
        return true
    }

    private static func index(of marker: [UInt16], in units: [UInt16], from: Int) -> Int? {
        guard !marker.isEmpty, from <= units.count - marker.count else { return nil }
        var i = from
        let last = units.count - marker.count
        while i <= last {
            if units[i] == marker[0], starts(units, marker, at: i) { return i }
            i += 1
        }
        return nil
    }

    private static func lineEnd(_ units: [UInt16], from: Int) -> Int {
        var i = from
        while i < units.count, units[i] != newline { i += 1 }
        return i
    }

    /// Index just after the string that starts at `i` with the quote `quote`.
    private static func stringEnd(_ units: [UInt16], _ i: Int, quote: UInt16, multiline: Bool) -> Int {
        var j = i + 1
        let n = units.count
        while j < n {
            let c = units[j]
            if c == backslash { j += 2 } else if c == quote { return j + 1 } else if c == newline && !multiline { return j } else { j += 1 }
        }
        return n
    }

    /// `0x1F`, `0b01`, `12`, `1_000`, `3.14`, `1e-9`, with a short suffix (`10px`, `1f`, `50%`).
    private static func numberEnd(_ units: [UInt16], _ i: Int) -> Int {
        let n = units.count
        if units[i] == 0x30, i + 1 < n {
            let radix = units[i + 1]
            if radix == ascii("x") || radix == ascii("X") {
                var j = i + 2
                while j < n, isHex(units[j]) || units[j] == 0x5F { j += 1 }
                if j > i + 2 { return j }
            } else if radix == ascii("b") || radix == ascii("B") {
                var j = i + 2
                while j < n, units[j] == 0x30 || units[j] == 0x31 || units[j] == 0x5F { j += 1 }
                if j > i + 2 { return j }
            }
        }
        var j = i + 1
        while j < n, isDigit(units[j]) || units[j] == 0x5F { j += 1 }
        if j + 1 < n, units[j] == ascii("."), isDigit(units[j + 1]) {
            j += 2
            while j < n, isDigit(units[j]) { j += 1 }
        }
        if j < n, units[j] == ascii("e") || units[j] == ascii("E") {
            var k = j + 1
            if k < n, units[k] == ascii("+") || units[k] == ascii("-") { k += 1 }
            if k < n, isDigit(units[k]) {
                while k < n, isDigit(units[k]) { k += 1 }
                j = k
            }
        }
        var suffix = 0
        while j < n, suffix < 3, isLetter(units[j]) || units[j] == ascii("%") {
            j += 1
            suffix += 1
        }
        return j
    }

    /// End of the run that starts at `i` with a character accepted by `first` and goes on with `rest`; nil when
    /// `i` does not start one.
    private static func run(_ units: [UInt16], _ i: Int, first: (UInt16) -> Bool, rest: (UInt16) -> Bool) -> Int? {
        guard i < units.count, first(units[i]) else { return nil }
        var j = i + 1
        while j < units.count, rest(units[j]) { j += 1 }
        return j
    }

    private static func string(_ units: [UInt16], _ from: Int, _ to: Int) -> String {
        String(decoding: units[from ..< to], as: UTF16.self)
    }

    // MARK: Scan

    private static func scan(_ units: [UInt16], _ grammar: Grammar) -> [CodeToken] {
        var out: [CodeToken] = []
        let n = units.count
        var i = 0
        var inTag = false
        let hash = ascii("#")
        let apostrophe = ascii("'")

        func push(_ start: Int, _ end: Int, _ kind: CodeTokenKind) {
            if end > start { out.append(CodeToken(start: start, end: min(end, n), kind: kind)) }
        }

        scan: while i < n {
            let c = units[i]

            for marker in grammar.line where starts(units, marker, at: i) {
                if grammar.hashAfterSpace, marker.first == hash, i > 0, !isBlank(units[i - 1]), units[i - 1] != newline { continue }
                let end = lineEnd(units, from: i)
                push(i, end, .comment)
                i = end
                continue scan
            }
            for (open, close) in grammar.block where starts(units, open, at: i) {
                let end = index(of: close, in: units, from: i + open.count).map { $0 + close.count } ?? n
                push(i, end, .comment)
                i = end
                continue scan
            }
            for quotes in grammar.triple where starts(units, quotes, at: i) {
                let end = index(of: quotes, in: units, from: i + 3).map { $0 + 3 } ?? n
                push(i, end, .string)
                i = end
                continue scan
            }

            if grammar.markup {
                if !inTag {
                    // `<name` or `</name`.
                    if c == ascii("<") {
                        let nameAt = i + 1 < n && units[i + 1] == ascii("/") ? i + 2 : i + 1
                        if let end = run(units, nameAt, first: isLetter, rest: { isWord($0) || isOne($0, of: ":.-") }) {
                            push(i, end, .tag)
                            i = end
                            inTag = true
                            continue
                        }
                    }
                    i += 1
                    continue
                }
                if c == ascii(">") || (c == ascii("/") && i + 1 < n && units[i + 1] == ascii(">")) {
                    let end = c == ascii(">") ? i + 1 : i + 2
                    push(i, end, .tag)
                    i = end
                    inTag = false
                    continue
                }
                if c == ascii("\"") || c == apostrophe {
                    let end = stringEnd(units, i, quote: c, multiline: true)
                    push(i, end, .string)
                    i = end
                    continue
                }
                if let end = run(units, i, first: { isLetter($0) || isOne($0, of: "_:@#") }, rest: { isWord($0) || isOne($0, of: ":.@#-") }) {
                    push(i, end, .attribute)
                    i = end
                    continue
                }
                i += 1
                continue
            }

            if grammar.quotes.contains(c) {
                if grammar.lifetimes, c == apostrophe {
                    // A character ('x', '\n', a pair of surrogates) closes right away; anything else is a lifetime.
                    let escaped = i + 1 < n && units[i + 1] == backslash
                    let closesAt = [2, 3].first { i + $0 < n && units[i + $0] == apostrophe }
                    if !escaped, closesAt == nil {
                        i += 1
                        continue
                    }
                }
                if grammar.quoteAfterBoundary, i > 0, !isBlank(units[i - 1]), !isOne(units[i - 1], of: "\n[{,:-") {
                    i += 1
                    continue
                }
                let end = stringEnd(units, i, quote: c, multiline: grammar.multiline.contains(c))
                var kind = CodeTokenKind.string
                if grammar.keys {
                    var k = end
                    while k < n, isBlank(units[k]) { k += 1 }
                    if k < n, units[k] == ascii(":") { kind = .property }
                }
                push(i, end, kind)
                i = end
                continue
            }

            if grammar.css {
                if c == hash {
                    // `#` and 3 to 8 hexadecimal digits, then the end of the word.
                    var j = i + 1
                    while j < n, j - i <= 8, isHex(units[j]) { j += 1 }
                    if j - i - 1 >= 3, j == n || !isWord(units[j]) {
                        push(i, j, .number)
                        i = j
                        continue
                    }
                }
                if c == ascii("@"), let end = run(units, i + 1, first: { isLetter($0) || $0 == 0x2D }, rest: { isLetter($0) || $0 == 0x2D }) {
                    push(i, end, .keyword)
                    i = end
                    continue
                }
            }

            if isDigit(c) {
                let end = numberEnd(units, i)
                push(i, end, .number)
                i = end
                continue
            }

            let wordEnd = grammar.css
                ? run(units, i, first: { isLetter($0) || $0 == 0x5F || $0 == 0x2D }, rest: { isWord($0) || $0 == 0x2D })
                : run(units, i, first: { isLetter($0) || $0 == 0x5F || $0 == 0x24 }, rest: { isWord($0) || $0 == 0x24 })
            if let end = wordEnd {
                let word = string(units, i, end)
                if grammar.keywords.contains(grammar.caseInsensitive ? word.lowercased() : word) {
                    push(i, end, .keyword)
                } else if grammar.literals.contains(word) {
                    push(i, end, .literal)
                } else {
                    var k = end
                    while k < n, units[k] == space { k += 1 }
                    let next: UInt16? = k < n ? units[k] : nil
                    if next == ascii("("), !grammar.css {
                        push(i, end, .function)
                    } else if grammar.keys, next == ascii(":") {
                        push(i, end, .property)
                    } else if grammar.keyEquals, next == ascii("=") {
                        push(i, end, .property)
                    }
                }
                i = end
                continue
            }
            i += 1
        }
        return out
    }

    // MARK: Markdown

    /// Headings, fences and inline code of a Markdown file.
    private static func markdownTokens(_ units: [UInt16]) -> [CodeToken] {
        var out: [CodeToken] = []
        let n = units.count
        let backtick = ascii("`")
        let tilde = ascii("~")
        var position = 0
        var fenced = false
        while position <= n {
            let end = lineEnd(units, from: position)
            var first = position
            while first < end, isBlank(units[first]) { first += 1 }
            let fence = first + 2 < end && (units[first] == backtick || units[first] == tilde)
                && units[first + 1] == units[first] && units[first + 2] == units[first]
            if fence {
                out.append(CodeToken(start: position, end: end, kind: .string))
                fenced.toggle()
            } else if fenced {
                if end > position { out.append(CodeToken(start: position, end: end, kind: .string)) }
            } else if isHeading(units, position, end) {
                out.append(CodeToken(start: position, end: end, kind: .keyword))
            } else {
                // `code`: a backtick, at least one other character, a backtick, on this line.
                var i = position
                while i < end {
                    if units[i] == backtick {
                        var j = i + 1
                        while j < end, units[j] != backtick { j += 1 }
                        if j < end, j > i + 1 {
                            out.append(CodeToken(start: i, end: j + 1, kind: .string))
                            i = j + 1
                            continue
                        }
                    }
                    i += 1
                }
            }
            position = end + 1
        }
        return out
    }

    /// One to six `#` then a blank.
    private static func isHeading(_ units: [UInt16], _ start: Int, _ end: Int) -> Bool {
        var i = start
        while i < end, units[i] == ascii("#") { i += 1 }
        let count = i - start
        return count >= 1 && count <= 6 && i < end && isBlank(units[i])
    }
}
