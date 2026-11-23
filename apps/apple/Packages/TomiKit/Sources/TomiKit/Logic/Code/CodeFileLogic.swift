import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/code/CodeFileLogic.kt.
// Pure logic of the code file screen: which attachments are code files, which grammar Android colours them with, what is
// allowed on screen, how a file is decoded and the in-file search. The colours themselves are in `CodeHighlighter.swift`
// (a hand-written tokenizer, where Android uses tree-sitter), with the per-line colour runs (`CodeLines.bucketRuns`).
// Offsets of `CodeDocument` and `CodeMatch` are UTF-16 code units (as in Kotlin, and as `NSRange` / UITextView use).

/// File extensions → grammar (fence language) that would colour them, plus the code extensions that have no grammar. A code
/// file without a grammar (kt, kts, rb, cpp, sql, toml, ...) still opens in the dedicated screen, in plain monospace text,
/// with the search.
public enum CodeLanguages {
    private static let grammarByExtension: [String: String] = [
        "rs": "rust",
        "js": "javascript", "mjs": "javascript", "cjs": "javascript", "jsx": "javascript",
        "ts": "typescript", "mts": "typescript", "cts": "typescript",
        "tsx": "tsx",
        "css": "css",
        "html": "html", "htm": "html",
        "json": "json",
        "py": "python",
        "java": "java",
        "sh": "bash", "bash": "bash", "zsh": "bash",
        "go": "go",
        "c": "c", "h": "c",
        "yaml": "yaml", "yml": "yaml",
        "md": "markdown", "markdown": "markdown",
        "php": "php", "phtml": "php",
    ]

    /// Code extensions the dedicated screen opens even though no grammar colours them.
    private static let uncoloured: Set<String> = [
        "kt", "kts", "rb", "cpp", "cc", "cxx", "hpp", "hh", "sql", "toml", "gradle", "swift", "cs", "lua", "dart", "scala",
    ]

    /// Lower-case extension of a file name, without the dot; nil when it has none (`Makefile`, `.env` counts as none).
    public static func extensionOf(_ fileName: String) -> String? {
        var name = Substring(fileName)
        if let slash = name.lastIndex(of: "/") { name = name[name.index(after: slash)...] }
        if let backslash = name.lastIndex(of: "\\") { name = name[name.index(after: backslash)...] }
        guard let dot = name.lastIndex(of: "."), dot != name.startIndex, name.index(after: dot) != name.endIndex else { return nil }
        return name[name.index(after: dot)...].lowercased()
    }

    public static func isCodeFile(_ fileName: String) -> Bool {
        guard let ext = extensionOf(fileName) else { return false }
        return grammarByExtension[ext] != nil || uncoloured.contains(ext)
    }

    /// The grammar (fence language) for `fileName`, or nil when there is none: the file is shown without colours.
    public static func grammarFor(_ fileName: String) -> String? {
        extensionOf(fileName).flatMap { grammarByExtension[$0] }
    }
}

/// One code file of an item: its attachment and the index of that attachment in the whole list (what the core is asked for).
public struct CodeFile: Hashable, Sendable {
    public let attachmentIndex: Int
    public let attachment: Attachment

    public init(attachmentIndex: Int, attachment: Attachment) {
        self.attachmentIndex = attachmentIndex
        self.attachment = attachment
    }

    public var grammar: String? { CodeLanguages.grammarFor(attachment.name) }
}

public enum CodeFiles {
    /// An image is never a code file, whatever its name.
    public static func isCodeFile(_ attachment: Attachment) -> Bool {
        !attachment.isImage && CodeLanguages.isCodeFile(attachment.name)
    }

    /// The code files of `item`, in order. A mixed item (images + code + pdf) only pages through its code files; each keeps the
    /// index of its attachment in the whole list, because `TomiCore.attachmentUri(itemId, index)` counts every attachment.
    public static func of(_ item: TomiItem) -> [CodeFile] {
        item.attachments.enumerated().compactMap { index, attachment in
            isCodeFile(attachment) ? CodeFile(attachmentIndex: index, attachment: attachment) : nil
        }
    }

    /// Position of the file showing the attachment `attachmentIndex`; the first one when it is not a code file of the item.
    public static func positionFor(_ files: [CodeFile], attachmentIndex: Int) -> Int {
        max(0, files.firstIndex { $0.attachmentIndex == attachmentIndex } ?? -1)
    }

    /// Attachment index of the file at `position` (coerced into range), or nil when the item has no code file.
    public static func attachmentIndexAt(_ files: [CodeFile], position: Int) -> Int? {
        if files.isEmpty { return nil }
        return files[min(max(position, 0), files.count - 1)].attachmentIndex
    }

    /// "2/3" for the position 1 of 3 (position is 0-based).
    public static func positionLabel(position: Int, total: Int) -> String {
        "\(min(max(position, 0), max(0, total - 1)) + 1)/\(total)"
    }
}

/// What the code screen is allowed to put on screen. A hidden ("Masqué") item shows nothing until the biometric gate passed.
public enum CodeContent: Hashable, Sendable {
    /// The item is not known yet.
    case loading
    /// The item is gone (deleted, expired) or has no code file.
    case empty
    /// A hidden item whose authentication has not succeeded: no file read, no decoding, no name.
    case locked
    case files([CodeFile])
}

public enum CodeGate {
    public static func content(item: TomiItem?, loaded: Bool, unlocked: Bool) -> CodeContent {
        guard let item else { return loaded ? .empty : .loading }
        if item.secret && !unlocked { return .locked }
        let files = CodeFiles.of(item)
        return files.isEmpty ? .empty : .files(files)
    }

    /// Whether the screen must close when the app leaves the screen: a hidden item never stays on screen. A rotation
    /// stops the screen too and does not close it (the gate is asked again after the re-creation, instant inside the window).
    public static func closesOnStop(item: TomiItem?, changingConfigurations: Bool) -> Bool {
        item?.secret == true && !changingConfigurations
    }
}

/// Limits of what is read and coloured.
public enum CodeLimits {
    /// Largest file that is read into memory (the screen says so beyond).
    public static let maxBytes = 2 * 1024 * 1024

    /// A block over 50 KB of UTF-8 or 2,000 lines stays uncoloured.
    public static let highlightMaxBytes = 50 * 1024
    public static let highlightMaxLines = 2000

    /// A line longer than this is cut on screen (a minified bundle on one line would exceed the layout limits).
    public static let maxDisplayLineChars = 4000

    /// Spaces a tab is shown as.
    public static let tabWidth = 4

    private static let binarySniffBytes = 8000

    /// `lineCount`: number of lines of `text`.
    public static func canHighlight(text: String, lineCount: Int) -> Bool {
        lineCount <= highlightMaxLines && text.utf16.count <= highlightMaxBytes && text.utf8.count <= highlightMaxBytes
    }

    /// A NUL byte in the first 8000 bytes.
    static func looksBinary(_ bytes: [UInt8]) -> Bool {
        bytes.prefix(binarySniffBytes).contains(0)
    }
}

/// Result of reading a file, before any display.
public enum CodeLoadResult: Hashable, Sendable {
    /// `original` is the text as stored (copied as is), `display` has uniform `\n` line ends and tabs expanded.
    case text(original: String, display: String)
    case tooLarge
    case binary
    case unavailable
}

public enum CodeDecoding {
    /// Decodes the bytes of a file. `bytes` must not be longer than `CodeLimits.maxBytes + 1` (the reader stops there).
    public static func decode(_ bytes: [UInt8]) -> CodeLoadResult {
        if bytes.count > CodeLimits.maxBytes { return .tooLarge }
        if CodeLimits.looksBinary(bytes) { return .binary }
        let start = bytes.count >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF ? 3 : 0
        // Invalid sequences become U+FFFD: a file in another encoding is shown, not refused.
        let original = String(decoding: bytes[start...], as: UTF8.self)
        return .text(original: original, display: display(original))
    }

    /// Reads a stream into memory and decodes it. Stops as soon as the file is known to be over `CodeLimits.maxBytes`, without
    /// reading the rest. Nothing is written anywhere: the text only lives in the memory of the screen.
    public static func read(_ input: InputStream) -> CodeLoadResult {
        input.open()
        defer { input.close() }
        var data = [UInt8]()
        var buffer = [UInt8](repeating: 0, count: 16 * 1024)
        while true {
            let n = input.read(&buffer, maxLength: buffer.count)
            if n < 0 { return .unavailable }
            if n == 0 { break }
            if data.count + n > CodeLimits.maxBytes { return .tooLarge }
            data.append(contentsOf: buffer[0 ..< n])
        }
        return decode(data)
    }

    /// `\r\n` and `\r` become `\n`; a tab becomes `CodeLimits.tabWidth` spaces. Search works on this text.
    public static func display(_ original: String) -> String {
        let unified = original
            .replacingOccurrences(of: "\r\n", with: "\n", options: .literal)
            .replacingOccurrences(of: "\r", with: "\n", options: .literal)
        return unified.replacingOccurrences(of: "\t", with: String(repeating: " ", count: CodeLimits.tabWidth), options: .literal)
    }
}

/// A text cut into lines, with offset ↔ (line, column) conversions. Lines are split on `\n` only (see `CodeDecoding.display`).
/// Offsets and columns are UTF-16 code units.
public struct CodeDocument: Sendable {
    public let text: String
    private let units: [UInt16]
    private let starts: [Int]

    public init(_ text: String) {
        self.text = text
        let units = Array(text.utf16)
        self.units = units
        var starts = [0]
        for (i, unit) in units.enumerated() where unit == 0x0A { starts.append(i + 1) }
        self.starts = starts
    }

    public var lineCount: Int { starts.count }

    /// Length of the text in UTF-16 code units.
    public var length: Int { units.count }

    public func lineStart(_ line: Int) -> Int { starts[line] }

    /// End of `line` (exclusive), the `\n` not included.
    public func lineEnd(_ line: Int) -> Int { line + 1 < starts.count ? starts[line + 1] - 1 : units.count }

    public func line(_ line: Int) -> String {
        String(decoding: units[starts[line] ..< lineEnd(line)], as: UTF16.self)
    }

    /// The line that contains the offset `offset` (the `\n` that ends a line belongs to it).
    public func lineOf(_ offset: Int) -> Int {
        let target = min(max(offset, 0), units.count)
        var low = 0
        var high = starts.count - 1
        while low < high {
            let mid = (low + high + 1) >> 1
            if starts[mid] <= target { low = mid } else { high = mid - 1 }
        }
        return low
    }

    public func columnOf(_ offset: Int) -> Int {
        min(max(offset, 0), units.count) - starts[lineOf(offset)]
    }

    public var maxLineLength: Int {
        var longest = 0
        for i in 0 ..< lineCount { longest = max(longest, lineEnd(i) - starts[i]) }
        return min(longest, CodeLimits.maxDisplayLineChars)
    }
}

/// An occurrence of the search text: `start` inclusive, `end` exclusive, UTF-16 offsets in the display text.
public struct CodeMatch: Hashable, Sendable {
    public let start: Int
    public let end: Int

    public init(start: Int, end: Int) {
        self.start = start
        self.end = end
    }
}

public enum CodeSearch {
    /// More occurrences than this are not listed (the counter then shows "n/5000+").
    public static let maxMatches = 5000

    /// Occurrences of `query` in `text`, case-insensitive (per UTF-16 unit, like Kotlin `ignoreCase`), in order, never
    /// overlapping. An empty query has none.
    public static func find(text: String, query: String) -> [CodeMatch] {
        let needle = Array(query.utf16).map(fold)
        if needle.isEmpty { return [] }
        let hay = Array(text.utf16).map(fold)
        var matches = [CodeMatch]()
        var from = 0
        let last = hay.count - needle.count
        while matches.count < maxMatches, from <= last {
            var at = from
            var found = -1
            while at <= last {
                if hay[at] == needle[0], hay[at ..< at + needle.count].elementsEqual(needle) {
                    found = at
                    break
                }
                at += 1
            }
            if found < 0 { break }
            matches.append(CodeMatch(start: found, end: found + needle.count))
            from = found + needle.count
        }
        return matches
    }

    private static func fold(_ unit: UInt16) -> UInt16 {
        if unit < 128 { return (unit >= 65 && unit <= 90) ? unit + 32 : unit }
        guard let scalar = Unicode.Scalar(unit) else { return unit }
        let lower = Array(String(scalar).uppercased().lowercased().utf16)
        return lower.count == 1 ? lower[0] : unit
    }

    /// Next occurrence, wrapping to the first one after the last. -1 when there is none.
    public static func next(current: Int, count: Int) -> Int {
        count <= 0 ? -1 : floorMod(current + 1, count)
    }

    /// Previous occurrence, wrapping to the last one before the first. -1 when there is none.
    public static func previous(current: Int, count: Int) -> Int {
        count <= 0 ? -1 : floorMod(current - 1, count)
    }

    private static func floorMod(_ a: Int, _ n: Int) -> Int { ((a % n) + n) % n }

    /// The occurrence to select when the search text changes: the first one at or after `fromOffset`, else the first one.
    public static func firstFrom(_ matches: [CodeMatch], fromOffset: Int) -> Int {
        if matches.isEmpty { return -1 }
        return matches.firstIndex { $0.start >= fromOffset } ?? 0
    }

    /// "3/12" (1-based) for the occurrence `current` of `count`; "0/0" when there is none; "3/5000+" past the listing cap.
    public static func counter(current: Int, count: Int) -> String {
        if count <= 0 || current < 0 { return "0/0" }
        return "\(min(current, count - 1) + 1)/\(count)" + (count >= maxMatches ? "+" : "")
    }
}

/// Line wrap of the code screen: off (default) = sideways scroll, on = lines fold to the screen width.
public enum CodeWrap {
    /// The other state.
    public static func toggled(_ enabled: Bool) -> Bool { !enabled }

    /// Width (px) left to the text of a line, once the gutter, the gap after it and the right padding are taken off the viewport.
    /// At least 1.
    public static func textWidthPx(viewportWidthPx: Int, gutterPx: Double, gapPx: Double, endPaddingPx: Double) -> Int {
        let width = (Double(viewportWidthPx) - gutterPx - gapPx - endPaddingPx).rounded(.towardZero)
        guard width.isFinite, width >= 1 else { return width == .infinity ? Int(Int32.max) : 1 }
        return width > Double(Int32.max) ? Int(Int32.max) : Int(width)
    }

    /// Distance (px, positive = content moves up) to scroll so that the row [rowTop, rowBottom) is comfortably in the viewport
    /// [viewportStart, viewportEnd) (all in the coordinates of the list): nil when it already is, else the row lands a third
    /// down the viewport. `margin` is the room kept above and below the row (capped at a quarter of the viewport).
    public static func scrollDelta(rowTop: Double, rowBottom: Double, viewportStart: Int, viewportEnd: Int, margin: Double) -> Double? {
        let height = Double(viewportEnd - viewportStart)
        if height <= 0 { return nil }
        let room = min(margin, height / 4)
        if rowTop >= Double(viewportStart) + room && rowBottom <= Double(viewportEnd) - room { return nil }
        return rowTop - (Double(viewportStart) + height / 3)
    }
}

/// An occurrence on one line (a match that crosses lines is cut at each line end). `index` is its rank in the list of matches.
public struct LineMatch: Hashable, Sendable {
    public let startCol: Int
    public let endCol: Int
    public let index: Int

    public init(startCol: Int, endCol: Int, index: Int) {
        self.startCol = startCol
        self.endCol = endCol
        self.index = index
    }
}

public enum CodeLines {
    /// Splits [start, end) at line ends and gives each piece to `block` with its line and columns. Empty pieces are skipped.
    public static func forEachLinePiece(_ doc: CodeDocument, start: Int, end: Int, _ block: (_ line: Int, _ startCol: Int, _ endCol: Int) -> Void) {
        let from = min(max(start, 0), doc.length)
        let to = min(max(end, from), doc.length)
        if to <= from { return }
        var line = doc.lineOf(from)
        var cursor = from
        while cursor < to && line < doc.lineCount {
            let lineStart = doc.lineStart(line)
            let lineEnd = doc.lineEnd(line)
            let pieceEnd = min(to, lineEnd)
            if pieceEnd > cursor { block(line, cursor - lineStart, pieceEnd - lineStart) }
            cursor = lineEnd + 1
            line += 1
        }
    }

    /// Occurrences grouped by line.
    public static func bucketMatches(_ doc: CodeDocument, matches: [CodeMatch]) -> [Int: [LineMatch]] {
        var result = [Int: [LineMatch]]()
        for (index, match) in matches.enumerated() {
            forEachLinePiece(doc, start: match.start, end: match.end) { line, a, b in
                result[line, default: []].append(LineMatch(startCol: a, endCol: b, index: index))
            }
        }
        return result
    }

    /// Horizontal position (in characters) to scroll to so that the column `col` is visible: nil when it already is.
    public static func horizontalScrollTarget(col: Int, visibleStartChar: Int, visibleChars: Int) -> Int? {
        if visibleChars <= 0 { return nil }
        let margin = min(8, visibleChars / 4)
        return col < visibleStartChar + margin || col > visibleStartChar + visibleChars - margin ? max(0, col - visibleChars / 3) : nil
    }
}
