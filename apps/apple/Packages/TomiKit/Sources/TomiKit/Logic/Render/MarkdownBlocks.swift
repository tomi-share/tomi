import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/render/MarkdownBlocks.kt: same functions, same rules, same
// thresholds.
//
// Deviations from Kotlin:
// - the top-level functions and types live in the `MarkdownBlocks` namespace (`preview`, `head`, `chunks`,
//   `sourceChunks`, `Budget`, `Preview`, `Chunk`): `Preview` alone would clash with SwiftUI's;
// - the text is read as UTF-16 code units (`[UInt16]`), so that every length, budget and offset counts what Kotlin
//   counts (a Swift `Character` is a whole grapheme cluster: "\r\n" or a family emoji would count for one);
// - the regular expressions are written out by hand (same languages, no backtracking);
// - `head` only reads the beginning of the text (twice the budget), never the whole of it.

/// Bounded previews and chunks of a long text (pure logic, tested on macOS).
///
/// A card shows 220 pt (about 10 lines) of its body, yet the Markdown renderer used to guard, parse and lay out the
/// WHOLE document, again at each evaluation of the card. The preview cuts the source BEFORE the guard, the parser and
/// the renderer: whole lines up to a budget, then adjusted so the cut never lands inside a construct that would render
/// differently (fenced code is closed, a table is only cut between two rows). The budget (40 lines / 2,000 characters)
/// is far beyond what the card shows, so an imperfect cut stays out of sight.
///
/// The preview and the chunks only live in memory, for display: opening, copying, sharing and searching still use the
/// full text.
public enum MarkdownBlocks {
    public struct Budget: Hashable, Sendable {
        public let maxChars: Int
        public let maxLines: Int
        public let maxTableRows: Int

        public init(maxChars: Int, maxLines: Int, maxTableRows: Int) {
            self.maxChars = maxChars
            self.maxLines = maxLines
            self.maxTableRows = maxTableRows
        }
    }

    /// Budget of a feed card: the rendered height stays around 2,000 pt at most, the card shows 220 pt of it.
    public static let cardBudget = Budget(maxChars: 2_000, maxLines: 40, maxTableRows: 8)

    /// A bounded text. Never prints the content (secrets, logs).
    public struct Preview: Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
        public let markdown: String
        public let truncated: Bool

        public init(markdown: String, truncated: Bool) {
            self.markdown = markdown
            self.truncated = truncated
        }

        public var description: String { "Preview(<redacted>, \(truncated))" }
        public var debugDescription: String { description }
        public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
    }

    /// Cap of the reference definitions (`[id]: url`) kept for free: those of the preview and those appended from
    /// further down.
    private static let freeDefinitionsCap = 8 * 1024

    /// A long line is cut inside only if this much room is left (else the preview simply stops before it).
    private static let minPartialChars = 40

    /// Markdown preview: `md` itself (not truncated) when it fits in `budget`, else its beginning, cut at a safe block
    /// boundary, followed by the reference definitions it uses. Idempotent: the preview of a preview is itself, not
    /// truncated.
    public static func preview(_ md: String, budget: Budget = cardBudget) -> Preview {
        if md.isEmpty { return Preview(markdown: md, truncated: false) }
        let s = Units(md.utf16)
        let lines = splitLines(s)
        if walk(s, lines, budget.maxLines, budget.maxChars, budget.maxTableRows).complete {
            return Preview(markdown: md, truncated: false)
        }
        // One line and one character are kept back for the blank line that separates the appended definitions.
        let cut = preferBlockBoundary(walk(s, lines, budget.maxLines - 1, budget.maxChars - 1, budget.maxTableRows))
        var body = Units(s[0..<cut.endOffset(lines)])
        trimTrailingBlank(&body)
        if let fence = cut.openFence {
            body.append(U.newline)
            body.append(contentsOf: fence.closing)
        }
        let definitions = laterDefinitions(s, lines, cut, body, freeDefinitionsCap - cut.freeUsed)
        if !definitions.isEmpty {
            body.append(U.newline)
            body.append(U.newline)
            appendJoined(definitions, to: &body)
        }
        return Preview(markdown: string(body), truncated: true)
    }

    /// First lines of a plain text (plain text, Markdown source shown literally, revealed secret): at most `maxLines`
    /// lines and `maxChars` characters, a too long line being cut on a space (never inside a surrogate pair or an
    /// obvious emoji cluster).
    public static func head(_ text: String, maxLines: Int, maxChars: Int) -> Preview {
        // A card only pays for what it keeps, whatever the length of the text: a line costs its length + 1 and takes
        // at most its length + 2 code units, so what is kept lies in the first 2 x maxChars units, and a line that
        // runs past them does not fit anyway.
        let cap = 2 * max(maxChars, 0) + 4
        var s = Units()
        s.reserveCapacity(min(cap, 4_096))
        var longer = false
        for unit in text.utf16 {
            if s.count == cap {
                longer = true
                break
            }
            s.append(unit)
        }
        var usedLines = 0
        var usedChars = 0
        var previousEnd = 0
        var start = 0
        let length = s.count
        while start < length {
            var end = start
            while end < length && s[end] != U.newline && s[end] != U.carriageReturn { end += 1 }
            if usedLines + 1 > maxLines { return Preview(markdown: string(trimmedEnd(s, 0, previousEnd)), truncated: true) }
            let cost = end - start + 1
            if usedChars + cost > maxChars {
                let room = maxChars - usedChars - 1
                let kept = room > 0 ? start + partialCut(s, Line(start: start, end: end), room) : previousEnd
                return Preview(markdown: string(trimmedEnd(s, 0, kept)), truncated: true)
            }
            usedLines += 1
            usedChars += cost
            previousEnd = end
            if end >= length {
                start = length
            } else if s[end] == U.carriageReturn && end + 1 < length && s[end + 1] == U.newline {
                start = end + 2
            } else {
                start = end + 1
            }
        }
        // Never reached with `longer` (see above); kept as a safe answer.
        if longer { return Preview(markdown: string(trimmedEnd(s, 0, previousEnd)), truncated: true) }
        return Preview(markdown: text, truncated: false)
    }

    // --- UTF-16 ----------------------------------------------------------------------------------------------------------

    typealias Units = [UInt16]

    private enum U {
        static let tab: UInt16 = 0x09
        static let newline: UInt16 = 0x0A
        static let carriageReturn: UInt16 = 0x0D
        static let space: UInt16 = 0x20
        static let bang: UInt16 = 0x21
        static let hash: UInt16 = 0x23
        static let closeParen: UInt16 = 0x29
        static let star: UInt16 = 0x2A
        static let plus: UInt16 = 0x2B
        static let dash: UInt16 = 0x2D
        static let dot: UInt16 = 0x2E
        static let slash: UInt16 = 0x2F
        static let colon: UInt16 = 0x3A
        static let lessThan: UInt16 = 0x3C
        static let greaterThan: UInt16 = 0x3E
        static let openBracket: UInt16 = 0x5B
        static let backslash: UInt16 = 0x5C
        static let closeBracket: UInt16 = 0x5D
        static let underscore: UInt16 = 0x5F
        static let backtick: UInt16 = 0x60
        static let pipe: UInt16 = 0x7C
        static let tilde: UInt16 = 0x7E
        static let zeroWidthJoiner: UInt16 = 0x200D
    }

    private static func string(_ units: Units) -> String { String(decoding: units, as: UTF16.self) }
    private static func string(_ units: ArraySlice<UInt16>) -> String { String(decoding: units, as: UTF16.self) }

    private static func units(_ text: String) -> Units { Units(text.utf16) }

    /// Kotlin's `Char.isWhitespace()`: the space separators, the line and paragraph separators, and the ASCII controls
    /// that separate (tab to carriage return, U+001C to U+001F).
    private static func isWhitespace(_ u: UInt16) -> Bool {
        switch u {
        case 0x09...0x0D, 0x1C...0x20, 0xA0, 0x1680, 0x2000...0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000: true
        default: false
        }
    }

    /// `\s` of the regular expressions, on a line (which holds no \n nor \r): space, tab, vertical tab, form feed.
    private static func isRegexSpace(_ u: UInt16) -> Bool { u == U.space || u == U.tab || u == 0x0B || u == 0x0C }

    /// A line terminator of the regular expressions other than \n and \r: `$` also matches just before one that ends
    /// the input, and `.` does not match one.
    private static func isRegexLineTerminator(_ u: UInt16) -> Bool { u == 0x85 || u == 0x2028 || u == 0x2029 }

    private static func isHighSurrogate(_ u: UInt16) -> Bool { u & 0xFC00 == 0xD800 }
    private static func isLowSurrogate(_ u: UInt16) -> Bool { u & 0xFC00 == 0xDC00 }

    /// `[start, end)` of `s` without its trailing whitespace.
    private static func trimmedEnd(_ s: Units, _ start: Int, _ end: Int) -> ArraySlice<UInt16> {
        var e = end
        while e > start && isWhitespace(s[e - 1]) { e -= 1 }
        return s[start..<e]
    }

    /// `[start, end)` of `s` without its leading and trailing whitespace (Kotlin `trim()`).
    private static func trimmed(_ s: Units, _ line: Line) -> Line {
        var a = line.start
        var b = line.end
        while a < b && isWhitespace(s[a]) { a += 1 }
        while b > a && isWhitespace(s[b - 1]) { b -= 1 }
        return Line(start: a, end: b)
    }

    private static func appendJoined(_ parts: [Units], to body: inout Units) {
        for (index, part) in parts.enumerated() {
            if index > 0 { body.append(U.newline) }
            body.append(contentsOf: part)
        }
    }

    // --- lines -----------------------------------------------------------------------------------------------------------

    /// A line of the source: its content is `[start, end)`, line separator excluded (\n, \r\n or \r). Also any range
    /// of a line.
    struct Line {
        let start: Int
        let end: Int
        var length: Int { end - start }
    }

    /// Lines of `s`; a final line separator does not open an extra empty line.
    private static func splitLines(_ s: Units) -> [Line] {
        var lines: [Line] = []
        var start = 0
        var i = 0
        let n = s.count
        while i < n {
            let c = s[i]
            if c == U.newline || c == U.carriageReturn {
                lines.append(Line(start: start, end: i))
                i += (c == U.carriageReturn && i + 1 < n && s[i + 1] == U.newline) ? 2 : 1
                start = i
            } else {
                i += 1
            }
        }
        if start < n { lines.append(Line(start: start, end: n)) }
        return lines
    }

    /// First character of `line` after an indent of 3 spaces at most, or NUL (empty line, deeper indent): what a fence
    /// or a reference definition starts with.
    private static func leadChar(_ md: Units, _ line: Line) -> UInt16 {
        var i = line.start
        let stop = min(line.end, line.start + 4)
        while i < stop && md[i] == U.space { i += 1 }
        return (i < line.end && i - line.start <= 3) ? md[i] : 0
    }

    private static func isBlank(_ s: Units, _ text: Line) -> Bool {
        var i = text.start
        while i < text.end {
            if s[i] != U.space && s[i] != U.tab { return false }
            i += 1
        }
        return true
    }

    private static func contains(_ s: Units, _ text: Line, _ unit: UInt16) -> Bool {
        var i = text.start
        while i < text.end {
            if s[i] == unit { return true }
            i += 1
        }
        return false
    }

    // --- block recognition (a CommonMark / GFM subset: enough to never cut a construct in two) ---------------------------

    /// A fence closes with its own indent: inside a list item, a fence at column 0 would end the item and open a new
    /// block.
    struct Fence {
        let char: UInt16
        let length: Int
        let indent: Int

        var closing: Units {
            Units(repeating: U.space, count: indent) + Units(repeating: char, count: length)
        }
    }

    /// Indentation of at most 3 spaces (a tab or 4 spaces make an indented code block, not a fence).
    private static func indentOk(_ s: Units, _ text: Line) -> Int? {
        var i = text.start
        while i < text.end && s[i] == U.space { i += 1 }
        let indent = i - text.start
        return indent <= 3 ? indent : nil
    }

    /// The run of fence characters at `from` (an offset in `s`): its character and its length, 3 at least.
    private static func fenceRun(_ s: Units, _ text: Line, _ from: Int) -> (char: UInt16, length: Int)? {
        if from >= text.end { return nil }
        let c = s[from]
        if c != U.backtick && c != U.tilde { return nil }
        var i = from
        while i < text.end && s[i] == c { i += 1 }
        return i - from >= 3 ? (c, i - from) : nil
    }

    /// Opening fence: ``` or ~~~ (3 or more); the info string of a backtick fence holds no backtick.
    private static func openingFence(_ s: Units, _ text: Line) -> Fence? {
        guard let indent = indentOk(s, text), let run = fenceRun(s, text, text.start + indent) else { return nil }
        if run.char == U.backtick {
            var i = text.start + indent + run.length
            while i < text.end {
                if s[i] == U.backtick { return nil }
                i += 1
            }
        }
        return Fence(char: run.char, length: run.length, indent: indent)
    }

    /// Closing fence: same character, at least as long, nothing but spaces after.
    private static func closes(_ s: Units, _ text: Line, _ fence: Fence) -> Bool {
        guard let indent = indentOk(s, text), let run = fenceRun(s, text, text.start + indent) else { return false }
        return run.char == fence.char && run.length >= fence.length
            && isBlank(s, Line(start: text.start + indent + run.length, end: text.end))
    }

    /// `^ {0,3}\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$`, the whole line.
    static func isDelimiterRow(_ s: Units, _ text: Line) -> Bool {
        let end = text.end
        var i = text.start
        func skipSpaces() { while i < end && isRegexSpace(s[i]) { i += 1 } }
        /// `:?-+:?\s*` at `i`; leaves `i` where it was when there is none.
        func cell() -> Bool {
            var j = i
            if j < end && s[j] == U.colon { j += 1 }
            let dashes = j
            while j < end && s[j] == U.dash { j += 1 }
            if j == dashes { return false }
            if j < end && s[j] == U.colon { j += 1 }
            i = j
            skipSpaces()
            return true
        }
        // A leading pipe comes after 3 spaces at most; without one, any whitespace leads to the first cell.
        var spaces = i
        while spaces < end && s[spaces] == U.space { spaces += 1 }
        if spaces < end && s[spaces] == U.pipe && spaces - i <= 3 { i = spaces + 1 }
        skipSpaces()
        if !cell() { return false }
        while i < end && s[i] == U.pipe {
            i += 1
            skipSpaces()
            if !cell() { break } // the trailing pipe
        }
        return i == end
    }

    /// `^ {0,3}#{1,6}(\s|$)`.
    private static func matchesHeading(_ s: Units, _ text: Line) -> Bool {
        var i = text.start
        while i < text.end && s[i] == U.space && i - text.start < 3 { i += 1 }
        let hashes = i
        while i < text.end && s[i] == U.hash { i += 1 }
        let count = i - hashes
        if count < 1 || count > 6 { return false }
        return atSpaceOrEnd(s, text, i)
    }

    /// `(\s|$)` at `i`: a whitespace, the end of the line, or the line terminator that ends it.
    private static func atSpaceOrEnd(_ s: Units, _ text: Line, _ i: Int, tabOrSpaceOnly: Bool = false) -> Bool {
        if i == text.end { return true }
        let c = s[i]
        if tabOrSpaceOnly ? (c == U.space || c == U.tab) : isRegexSpace(c) { return true }
        return i == text.end - 1 && isRegexLineTerminator(c)
    }

    /// `^ {0,3}>`.
    private static func matchesQuote(_ s: Units, _ text: Line) -> Bool {
        var i = text.start
        while i < text.end && s[i] == U.space && i - text.start < 3 { i += 1 }
        return i < text.end && s[i] == U.greaterThan
    }

    /// Cells of a table row, split on unescaped pipes (leading and trailing pipes are borders).
    private static func cellCount(_ s: Units, _ text: Line) -> Int {
        let t = trimmed(s, text)
        var a = t.start
        var b = t.end
        if a < b && s[a] == U.pipe { a += 1 }
        if b > a && s[b - 1] == U.pipe && !(b - 2 >= a && s[b - 2] == U.backslash) { b -= 1 }
        var count = 1
        var i = a
        while i < b {
            if s[i] == U.backslash {
                i += 1
            } else if s[i] == U.pipe {
                count += 1
            }
            i += 1
        }
        return count
    }

    /// GFM table start: a header row with a pipe, then a delimiter row (`|---|`) with as many cells.
    private static func isTableStart(_ s: Units, _ header: Line, _ delimiter: Line) -> Bool {
        contains(s, header, U.pipe) && !isBlank(s, header) && isDelimiterRow(s, delimiter)
            && cellCount(s, header) == cellCount(s, delimiter)
    }

    /// A line that ends a table instead of being one of its rows.
    private static func endsTable(_ s: Units, _ text: Line) -> Bool {
        if isBlank(s, text) || openingFence(s, text) != nil { return true }
        switch firstNonSpace(s, text) {
        case U.hash: return matchesHeading(s, text)
        case U.greaterThan: return matchesQuote(s, text)
        default: return false
        }
    }

    /// `^ {0,3}\[((?:[^\[\]\\]|\\.){1,999})\]:\s*\S`: the label of the reference definition on this line, normalized.
    private static func definitionLabel(_ s: Units, _ text: Line) -> Units? {
        var i = text.start
        while i < text.end && s[i] == U.space && i - text.start < 3 { i += 1 }
        guard i < text.end, s[i] == U.openBracket else { return nil }
        i += 1
        let labelStart = i
        var count = 0
        while true {
            guard i < text.end else { return nil }
            let c = s[i]
            if c == U.closeBracket { break }
            if c == U.openBracket { return nil }
            if c == U.backslash {
                guard i + 1 < text.end, !isRegexLineTerminator(s[i + 1]) else { return nil }
                i += 1
            }
            // One code point (the regular expression counts them, not the code units).
            i += (isHighSurrogate(s[i]) && i + 1 < text.end && isLowSurrogate(s[i + 1])) ? 2 : 1
            count += 1
            if count > 999 { return nil }
        }
        guard count >= 1, i + 1 < text.end, s[i + 1] == U.colon else { return nil }
        var j = i + 2
        while j < text.end && isRegexSpace(s[j]) { j += 1 }
        guard j < text.end else { return nil }
        return normalizeLabel(s, labelStart, i)
    }

    /// Label as CommonMark matches it: trimmed, inner whitespace runs as one space, lowercase.
    private static func normalizeLabel(_ s: Units, _ start: Int, _ end: Int) -> Units {
        var out = Units()
        out.reserveCapacity(end - start)
        var pendingSpace = false
        var ascii = true
        var i = start
        while i < end {
            let c = s[i]
            if c == U.space || c == U.tab || c == U.newline || c == U.carriageReturn || c == 0x0B || c == 0x0C {
                pendingSpace = !out.isEmpty
            } else {
                if pendingSpace { out.append(U.space) }
                pendingSpace = false
                if c >= 0x80 { ascii = false }
                out.append(c >= 0x41 && c <= 0x5A ? c + 0x20 : c)
            }
            i += 1
        }
        return ascii ? out : units(string(out).lowercased())
    }

    // --- the walk --------------------------------------------------------------------------------------------------------

    /// Where a walk stopped. `lineCount` whole lines are kept, plus the first `partial` characters of the next one if
    /// not nil; `openFence` must then be closed; `freeUsed` = characters of the definitions kept for free.
    private struct Cut {
        let complete: Bool
        let lineCount: Int
        let partial: Int?
        let openFence: Fence?
        let freeUsed: Int
        let inTable: Bool
        /// For each kept line: chars used before it.
        let charsBefore: [Int]
        let usedChars: Int
        /// Lines before which a cut lands between two blocks.
        let blockBoundaries: [Int]

        func endOffset(_ lines: [Line]) -> Int {
            if let partial { return lines[lineCount].start + partial }
            return lineCount == 0 ? 0 : lines[lineCount - 1].end
        }
    }

    /// Takes whole lines while they fit in `maxLines` / `maxChars` (each line costs its length + 1), room being kept for
    /// the fence to close. A table is kept from its delimiter row on, at most `maxTableRows` rows; reference
    /// definitions are free up to the cap.
    private static func walk(_ md: Units, _ lines: [Line], _ maxLines: Int, _ maxChars: Int, _ maxTableRows: Int) -> Cut {
        var usedLines = 0
        var usedChars = 0
        var freeUsed = 0
        var fence: Fence?
        var inTable = false
        var tableRows = 0
        var charsBefore = [Int](repeating: 0, count: lines.count + 1)
        var boundaries: [Int] = []
        var i = 0

        func stop(_ partial: Int? = nil) -> Cut {
            charsBefore[i] = usedChars
            return Cut(
                complete: false, lineCount: i, partial: partial, openFence: fence, freeUsed: freeUsed, inTable: inTable,
                charsBefore: charsBefore, usedChars: usedChars, blockBoundaries: boundaries
            )
        }
        func closingCost(_ after: Fence?) -> Int { after.map { $0.indent + $0.length + 1 } ?? 0 }
        func fits(_ lineCost: Int, _ charCost: Int, _ after: Fence?) -> Bool {
            let reserveLines = after != nil ? 1 : 0
            return usedLines + lineCost + reserveLines <= maxLines && usedChars + charCost + closingCost(after) <= maxChars
        }
        func take(_ line: Line, free: Bool = false) {
            charsBefore[i] = usedChars
            if free {
                freeUsed += line.length + 1
            } else {
                usedLines += 1
                usedChars += line.length + 1
            }
            i += 1
        }
        /// Room left for part of a too long line (nil: not worth cutting inside it).
        func partialRoom(_ after: Fence?) -> Int? {
            if !fits(1, 0, after) { return nil }
            let room = maxChars - usedChars - 1 - closingCost(after)
            return room >= minPartialChars ? room : nil
        }

        while i < lines.count {
            let line = lines[i]
            if let open = fence {
                let after: Fence? = closes(md, line, open) ? nil : open
                if !fits(1, line.length + 1, after) {
                    var partial: Int?
                    if after != nil, let room = partialRoom(open) {
                        let kept = partialCut(md, line, room)
                        if kept > 0 && !closes(md, Line(start: line.start, end: line.start + kept), open) { partial = kept }
                    }
                    return stop(partial)
                }
                take(line)
                fence = after
                if after == nil { boundaries.append(i) }
                continue
            }
            if inTable {
                if !endsTable(md, line) {
                    if tableRows >= maxTableRows || !fits(1, line.length + 1, nil) { return stop() }
                    take(line)
                    tableRows += 1
                    continue
                }
                inTable = false
                boundaries.append(i)
            }
            if let opening = openingFence(md, line) {
                if !fits(1, line.length + 1, opening) { return stop() }
                if i > 0 { boundaries.append(i) }
                take(line)
                fence = opening
                continue
            }
            if i + 1 < lines.count && isTableStart(md, line, lines[i + 1]) {
                let next = lines[i + 1]
                if !fits(2, line.length + 1 + next.length + 1, nil) { return stop() }
                if i > 0 { boundaries.append(i) }
                take(line)
                take(next)
                inTable = true
                tableRows = 0
                continue
            }
            let isDefinition = leadChar(md, line) == U.openBracket && definitionLabel(md, line) != nil
            if isDefinition && freeUsed + line.length + 1 <= freeDefinitionsCap {
                take(line, free: true)
                continue
            }
            if !fits(1, line.length + 1, nil) {
                var partial: Int?
                if !isBlank(md, line), let room = partialRoom(nil) {
                    let kept = partialCut(md, line, room)
                    // Never inside a reference definition: its prefix would define the label with a truncated URL.
                    if kept > 0 && !isDefinition && openingFence(md, Line(start: line.start, end: line.start + kept)) == nil {
                        partial = kept
                    }
                }
                return stop(partial)
            }
            if isBlank(md, line) { boundaries.append(i) }
            take(line)
        }
        charsBefore[i] = usedChars
        return Cut(
            complete: true, lineCount: i, partial: nil, openFence: fence, freeUsed: freeUsed, inTable: inTable,
            charsBefore: charsBefore, usedChars: usedChars, blockBoundaries: boundaries
        )
    }

    /// A cut that stopped mid-block moves back to the last blank line (or other block boundary) when that keeps at
    /// least 3/4 of the lines and characters: the preview then ends on a whole paragraph. A cut between two table rows
    /// or inside a long line stays.
    private static func preferBlockBoundary(_ cut: Cut) -> Cut {
        if cut.complete || cut.partial != nil || cut.inTable || cut.lineCount == 0 { return cut }
        guard let best = cut.blockBoundaries.last(where: { at in
            at >= 1 && at < cut.lineCount && at * 4 >= cut.lineCount * 3 && cut.charsBefore[at] * 4 >= cut.usedChars * 3
        }) else { return cut }
        // A boundary is never inside fenced code (lines in a fence are not boundaries, a closing fence is one AFTER it).
        return Cut(
            complete: false, lineCount: best, partial: nil, openFence: nil, freeUsed: cut.freeUsed, inTable: false,
            charsBefore: cut.charsBefore, usedChars: cut.charsBefore[best], blockBoundaries: cut.blockBoundaries
        )
    }

    private static func trimTrailingBlank(_ body: inout Units) {
        var end = body.count
        while end > 0 && isWhitespace(body[end - 1]) { end -= 1 }
        body.removeSubrange(end..<body.count)
    }

    /// Reference definitions found after the cut (outside fenced code) whose label the preview uses and does not
    /// define itself, first definition of a label only (CommonMark), within `cap` characters.
    private static func laterDefinitions(_ md: Units, _ lines: [Line], _ cut: Cut, _ body: Units, _ cap: Int) -> [Units] {
        if cap <= 0 { return [] }
        if !body.contains(U.openBracket) { return [] }
        /// What the preview holds between brackets, lowercase: built at the first definition that needs it.
        var bracketed: Set<Units>?
        // Labels the preview defines itself (and therefore already resolves), then those already appended.
        var defined = Set<Units>()
        var fence: Fence?
        var result: [Units] = []
        var used = 0
        for (index, line) in lines.enumerated() {
            // This runs over the whole document for each card: only a line that starts with a fence character or a
            // bracket can matter, the others are skipped.
            let lead = leadChar(md, line)
            if let open = fence {
                if lead == open.char && closes(md, line, open) { fence = nil }
                continue
            }
            if lead != U.openBracket && lead != U.backtick && lead != U.tilde { continue }
            if let opening = openingFence(md, line) {
                fence = opening
                continue
            }
            guard let label = definitionLabel(md, line) else { continue }
            if index < cut.lineCount {
                defined.insert(label)
                continue
            }
            if !defined.insert(label).inserted { continue }
            if bracketed == nil { bracketed = bracketedTexts(units(string(body).lowercased())) }
            if bracketed?.contains(label) != true { continue }
            let text = trimmed(md, line)
            if used + text.length + 1 > cap { continue }
            used += text.length + 1
            result.append(Units(md[text.start..<text.end]))
        }
        return result
    }

    /// Every `x` such that `[x]` is in `body` and `x` could be a label (its brackets escaped): the label `l` is used by
    /// the body when `[l]` is in it, which is then a lookup instead of a search of the body per definition.
    private static func bracketedTexts(_ body: Units) -> Set<Units> {
        var found = Set<Units>()
        let n = body.count
        // A label is 999 code points at most.
        let longest = 2 * 999
        for p in 0..<n where body[p] == U.openBracket {
            var k = p + 1
            let stop = min(n, p + 1 + longest + 1)
            while k < stop {
                let c = body[k]
                if c == U.closeBracket {
                    found.insert(Units(body[(p + 1)..<k]))
                    if body[k - 1] != U.backslash { break }
                } else if c == U.openBracket && body[k - 1] != U.backslash {
                    break
                }
                k += 1
            }
        }
        return found
    }

    // --- cutting inside a line -------------------------------------------------------------------------------------------

    /// Length of the prefix of `line` kept within `room` characters: on the last space of its second half, else on a
    /// safe boundary.
    private static func partialCut(_ s: Units, _ line: Line, _ room: Int) -> Int {
        if room <= 0 { return 0 }
        if room >= line.length { return line.length }
        let limit = line.start + room
        var space = -1
        var j = limit
        while j > line.start + room / 2 {
            if s[j] == U.space || s[j] == U.tab {
                space = j
                break
            }
            j -= 1
        }
        let end = space > line.start ? space : safeBoundary(s, limit, floor: line.start)
        var trimmed = end
        while trimmed > line.start && (s[trimmed - 1] == U.space || s[trimmed - 1] == U.tab) { trimmed -= 1 }
        return trimmed - line.start
    }

    /// Moves `at` back (not below `floor`) until it no longer splits a surrogate pair or an obvious emoji / combining
    /// cluster.
    static func safeBoundary(_ s: Units, _ at: Int, floor: Int = 0) -> Int {
        var p = min(max(at, floor), s.count)
        while p > floor && p < s.count && !isBoundary(s, p) { p -= 1 }
        return p
    }

    private static func codePoint(_ s: Units, at p: Int) -> UInt32 {
        let c = s[p]
        if isHighSurrogate(c), p + 1 < s.count, isLowSurrogate(s[p + 1]) {
            return 0x10000 + ((UInt32(c) - 0xD800) << 10) + (UInt32(s[p + 1]) - 0xDC00)
        }
        return UInt32(c)
    }

    private static func codePoint(_ s: Units, before p: Int) -> UInt32 {
        let c = s[p - 1]
        if isLowSurrogate(c), p - 2 >= 0, isHighSurrogate(s[p - 2]) {
            return 0x10000 + ((UInt32(s[p - 2]) - 0xD800) << 10) + (UInt32(c) - 0xDC00)
        }
        return UInt32(c)
    }

    private static func isBoundary(_ s: Units, _ p: Int) -> Bool {
        let c = s[p]
        let prev = s[p - 1]
        if isLowSurrogate(c) && isHighSurrogate(prev) { return false }
        if prev == U.zeroWidthJoiner || c == U.zeroWidthJoiner { return false }
        if (c >= 0xFE00 && c <= 0xFE0F) || c == 0x20E3 { return false } // variation selectors, keycap
        let cp = codePoint(s, at: p)
        if cp >= 0x1F3FB && cp <= 0x1F3FF { return false } // skin tone modifiers
        if cp >= 0xE0020 && cp <= 0xE007F { return false } // tag sequences (subdivision flags)
        if cp >= 0x300, let scalar = Unicode.Scalar(cp) {
            switch scalar.properties.generalCategory {
            case .nonspacingMark, .enclosingMark, .spacingMark: return false
            default: break
            }
        }
        if isRegionalIndicator(cp) {
            // Flags are pairs of regional indicators: a cut after an odd number of them splits a flag.
            var count = 0
            var q = p
            while q > 0 {
                let before = codePoint(s, before: q)
                if !isRegionalIndicator(before) { break }
                count += 1
                q -= before > 0xFFFF ? 2 : 1
            }
            if count % 2 == 1 { return false }
        }
        return true
    }

    private static func isRegionalIndicator(_ cp: UInt32) -> Bool { cp >= 0x1F1E6 && cp <= 0x1F1FF }

    // --- chunks of the detail --------------------------------------------------------------------------------------------
    //
    // The detail screen shows the whole text. Given to the renderer in one piece, a long Markdown document is parsed,
    // styled and laid out at once (a 45 KB file of tables = thousands of text views built on the main thread, a height
    // of tens of thousands of points). The detail renders it as a column of chunks instead, each its own view, built
    // while scrolling.
    //
    // A chunk is a run of whole top-level blocks. The source is only cut where the two halves render as the whole did:
    // on a blank line followed by a line at column 0 (never inside a list item, indented code or other nested content),
    // outside fenced code and outside the HTML blocks that run past blank lines. An ordered list is never cut (the
    // renderer numbers every list from 1). A block too big for one chunk is cut further: a table between two rows (its
    // header repeated), fenced code between two lines (closed, then reopened with its own opening line), a tight bullet
    // list between two top-level items. A block that cannot be cut and is still very long is shown as plain text (its
    // exact content), not rendered.
    //
    // The chunks only live in memory, for display: copying, sharing and searching still use the full text.

    /// Budget of one chunk of the detail: about a screen or two of rendered text.
    public static let chunkBudget = Budget(maxChars: 4_000, maxLines: 100, maxTableRows: 30)

    /// A block that cannot be cut and is longer than this is shown as plain text instead of being parsed and rendered.
    private static let unsplittableSourceChars = 16_000

    /// A chunk of a Markdown document: `markdown` is rendered on its own (or shown as plain text when `asSource`).
    /// Never prints the content (secrets, logs).
    ///
    /// Bookkeeping, for the tests (UTF-16 offsets): the chunk carries the source range `[sourceStart, sourceEnd)` at
    /// `prefixLength` in `markdown` (a repeated table header or a reopened fence comes before it), and `bodyLength`
    /// characters of `markdown` precede the appended reference definitions. `oversized`: the chunk could not be cut
    /// within the budget.
    public struct Chunk: Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
        public let markdown: String
        public let asSource: Bool
        let sourceStart: Int
        let sourceEnd: Int
        let prefixLength: Int
        let bodyLength: Int
        let oversized: Bool

        init(
            markdown: String, asSource: Bool, sourceStart: Int, sourceEnd: Int, prefixLength: Int, bodyLength: Int,
            oversized: Bool
        ) {
            self.markdown = markdown
            self.asSource = asSource
            self.sourceStart = sourceStart
            self.sourceEnd = sourceEnd
            self.prefixLength = prefixLength
            self.bodyLength = bodyLength
            self.oversized = oversized
        }

        /// A chunk that is `markdown` whole.
        init(markdown: String, asSource: Bool) {
            let length = markdown.utf16.count
            self.init(
                markdown: markdown, asSource: asSource, sourceStart: 0, sourceEnd: length, prefixLength: 0,
                bodyLength: length, oversized: false
            )
        }

        public var description: String { "Chunk(<redacted>)" }
        public var debugDescription: String { description }
        public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
    }

    private static let htmlRawTags: [Units] = ["pre", "script", "style", "textarea"].map { Units($0.utf16) }

    private static let splitNone: UInt8 = 0
    private static let splitFence: UInt8 = 1
    private static let splitTable: UInt8 = 2
    private static let splitList: UInt8 = 3

    /// The first character that is neither a space nor a tab, or 0.
    private static func firstNonSpace(_ s: Units, _ text: Line) -> UInt16 {
        var i = text.start
        while i < text.end {
            if s[i] != U.space && s[i] != U.tab { return s[i] }
            i += 1
        }
        return 0
    }

    /// `^ {0,3}\d{1,9}[.)](?:[ \t]|$)`.
    private static func isOrderedItem(_ s: Units, _ text: Line) -> Bool {
        var i = text.start
        while i < text.end && s[i] == U.space && i - text.start < 3 { i += 1 }
        let digits = i
        while i < text.end && s[i] >= 0x30 && s[i] <= 0x39 { i += 1 }
        let count = i - digits
        if count < 1 || count > 9 { return false }
        guard i < text.end, s[i] == U.dot || s[i] == U.closeParen else { return false }
        return atSpaceOrEnd(s, text, i + 1, tabOrSpaceOnly: true)
    }

    private static func isHeading(_ s: Units, _ text: Line) -> Bool {
        firstNonSpace(s, text) == U.hash && matchesHeading(s, text)
    }

    /// `^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$`, the whole line.
    private static func isThematicBreak(_ s: Units, _ text: Line) -> Bool {
        var i = text.start
        while i < text.end && s[i] == U.space && i - text.start < 3 { i += 1 }
        guard i < text.end else { return false }
        let marker = s[i]
        if marker != U.dash && marker != U.star && marker != U.underscore { return false }
        var count = 0
        while i < text.end {
            if s[i] == marker {
                count += 1
            } else if s[i] != U.space && s[i] != U.tab {
                return false
            }
            i += 1
        }
        return count >= 3
    }

    /// A bullet list item at column 0 with some content ("-", "+" or "*", then a space): `^[-+*][ \t]+\S`.
    private static func isBulletItem(_ s: Units, _ text: Line) -> Bool {
        if text.length == 0 { return false }
        let c = s[text.start]
        if c != U.dash && c != U.plus && c != U.star { return false }
        var i = text.start + 1
        while i < text.end && (s[i] == U.space || s[i] == U.tab) { i += 1 }
        if i == text.start + 1 || i >= text.end { return false }
        return !isRegexSpace(s[i])
    }

    /// The first 12 code units after the indent, ASCII letters in lowercase.
    private static func lowercasedHead(_ s: Units, _ from: Int, _ end: Int) -> Units {
        var head = Units()
        var i = from
        let stop = min(end, from + 12)
        while i < stop {
            let c = s[i]
            head.append(c >= 0x41 && c <= 0x5A ? c + 0x20 : c)
            i += 1
        }
        return head
    }

    private static func isLetter(_ u: UInt16) -> Bool {
        if u < 0x80 { return (u >= 0x41 && u <= 0x5A) || (u >= 0x61 && u <= 0x7A) }
        guard let scalar = Unicode.Scalar(u) else { return false }
        switch scalar.properties.generalCategory {
        case .uppercaseLetter, .lowercaseLetter, .titlecaseLetter, .modifierLetter, .otherLetter: return true
        default: return false
        }
    }

    private static let htmlCommentEnd = Units("-->".utf16)
    private static let htmlCdataEnd = Units("]]>".utf16)
    private static let htmlProcessingEnd = Units("?>".utf16)
    private static let htmlDeclarationEnd = Units(">".utf16)
    private static let htmlCommentStart = Units("<!--".utf16)
    private static let htmlCdataStart = Units("<![cdata[".utf16)
    private static let htmlProcessingStart = Units("<?".utf16)

    /// End marker of an HTML block that runs past blank lines (CommonMark types 1 to 5) starting on `text`, else nil.
    private static func htmlBlockEnd(_ s: Units, _ text: Line) -> Units? {
        guard let indent = indentOk(s, text) else { return nil }
        let from = text.start + indent
        if from >= text.end || s[from] != U.lessThan { return nil }
        let head = lowercasedHead(s, from, text.end)
        if head.starts(with: htmlCommentStart) { return htmlCommentEnd }
        if head.starts(with: htmlCdataStart) { return htmlCdataEnd }
        if head.starts(with: htmlProcessingStart) { return htmlProcessingEnd }
        if head.count > 2 && head[1] == U.bang && isLetter(head[2]) { return htmlDeclarationEnd }
        for tag in htmlRawTags where head.count > tag.count && head[1...tag.count].elementsEqual(tag) {
            let after: UInt16? = head.count > tag.count + 1 ? head[tag.count + 1] : nil
            if after == nil || after == U.space || after == U.tab || after == U.greaterThan {
                return Units("</".utf16) + tag + [U.greaterThan]
            }
        }
        return nil
    }

    /// Start of an HTML block that ends at the next blank line (CommonMark types 6 and 7, loosely: any tag at the
    /// start).
    private static func startsHtmlUntilBlank(_ s: Units, _ text: Line) -> Bool {
        guard let indent = indentOk(s, text) else { return false }
        let from = text.start + indent
        if from + 1 >= text.end || s[from] != U.lessThan { return false }
        let next = s[from + 1]
        return isLetter(next) || next == U.slash
    }

    /// Kotlin's comparison of two characters ignoring case, `needle` being ASCII lowercase: the long s is an "s", the
    /// dotless and the dotted capital i are an "i".
    private static func equalsIgnoringCase(_ c: UInt16, _ needle: UInt16) -> Bool {
        if c == needle { return true }
        guard needle >= 0x61 && needle <= 0x7A else { return false }
        if c == needle - 0x20 { return true }
        if needle == 0x73 { return c == 0x17F }
        if needle == 0x69 { return c == 0x130 || c == 0x131 }
        return false
    }

    /// `needle` (ASCII lowercase) is in `text` from the offset `from` on, ignoring case.
    private static func containsIgnoringCase(_ s: Units, _ text: Line, _ needle: Units, from: Int = 0) -> Bool {
        var i = text.start + max(from, 0)
        let last = text.end - needle.count
        while i <= last {
            var k = 0
            while k < needle.count && equalsIgnoringCase(s[i + k], needle[k]) { k += 1 }
            if k == needle.count { return true }
            i += 1
        }
        return false
    }

    /// A line that starts a block of its own (it ends a list item it is not indented into).
    private static func opensBlock(_ s: Units, _ text: Line) -> Bool {
        openingFence(s, text) != nil || isHeading(s, text) || isThematicBreak(s, text) || isBulletItem(s, text)
            || (firstNonSpace(s, text) == U.greaterThan && matchesQuote(s, text)) || startsHtmlUntilBlank(s, text)
    }

    /// What a single pass over the source learns, per line: may a chunk start here (`cut`), is it a heading, a table
    /// body row, where an oversized block may be cut further (`splitKind`, `splitRef` = the opening fence line or the
    /// table header line).
    private struct Outline {
        let md: Units
        let lines: [Line]
        let n: Int
        var cut: [Bool]
        var heading: [Bool]
        var row: [Bool]
        var splitKind: [UInt8]
        var splitRef: [Int]

        /// Label -> line of its first reference definition.
        var definitions: [Units: Int] = [:]

        /// Prefix sums of the line costs (length + 1) and of the table rows.
        var charsBefore: [Int]
        var rowsBefore: [Int]

        /// The source ends inside fenced code or a raw HTML block: nothing may be appended after its last line.
        var openAtEnd = false

        init(_ md: Units, _ lines: [Line]) {
            self.md = md
            self.lines = lines
            let n = lines.count
            self.n = n
            cut = [Bool](repeating: false, count: n)
            heading = [Bool](repeating: false, count: n)
            row = [Bool](repeating: false, count: n)
            splitKind = [UInt8](repeating: splitNone, count: n)
            splitRef = [Int](repeating: 0, count: n)
            charsBefore = [Int](repeating: 0, count: n + 1)
            rowsBefore = [Int](repeating: 0, count: n + 1)

            var fence: Fence?
            var fenceLine = -1
            var fenceSplittable = false
            var htmlEnd: Units?
            var htmlUntilBlank = false
            var tableHeader = -1
            var tableSplittable = false
            var orderedOpen = false
            var prevBlank = true
            var seenContent = false
            for i in 0..<n {
                let text = lines[i]
                if let open = fence {
                    if closes(md, text, open) {
                        fence = nil
                    } else if fenceSplittable && i >= fenceLine + 2 {
                        splitKind[i] = splitFence
                        splitRef[i] = fenceLine
                    }
                    prevBlank = false
                } else if let end = htmlEnd {
                    if containsIgnoringCase(md, text, end) { htmlEnd = nil }
                    prevBlank = false
                } else if isBlank(md, text) {
                    tableHeader = -1
                    htmlUntilBlank = false
                    prevBlank = true
                } else {
                    let first = md[text.start]
                    let col0 = first != U.space && first != U.tab
                    let ordered = isOrderedItem(md, text)
                    // A chunk starts on a blank line followed by column 0, never on the next item of an open ordered
                    // list.
                    if seenContent && prevBlank && col0 && !(ordered && orderedOpen) { cut[i] = true }
                    seenContent = true
                    if col0 && isHeading(md, text) { heading[i] = true }
                    var consumed = false
                    if tableHeader >= 0 {
                        if i == tableHeader + 1 {
                            consumed = true // delimiter row
                        } else if !endsTable(md, text) {
                            row[i] = true
                            if tableSplittable && i >= tableHeader + 3 {
                                splitKind[i] = splitTable
                                splitRef[i] = tableHeader
                            }
                            consumed = true
                        } else {
                            tableHeader = -1
                        }
                    }
                    if !consumed {
                        if ordered {
                            orderedOpen = true
                        } else if orderedOpen && col0 && (prevBlank || opensBlock(md, text)) {
                            orderedOpen = false
                        }
                        // Between two top-level items of a bullet list (never a thematic break such as "* * *").
                        if col0 && !orderedOpen && !htmlUntilBlank && !prevBlank && isBulletItem(md, text)
                            && !isThematicBreak(md, text) {
                            splitKind[i] = splitList
                        }
                        let opening = openingFence(md, text)
                        let rawEnd = opening == nil ? htmlBlockEnd(md, text) : nil
                        if let opening {
                            fence = opening
                            fenceLine = i
                            fenceSplittable = !orderedOpen && !htmlUntilBlank
                        } else if let rawEnd {
                            let from = (indentOk(md, text) ?? 0) + 2
                            if !containsIgnoringCase(md, text, rawEnd, from: from) { htmlEnd = rawEnd }
                        } else if startsHtmlUntilBlank(md, text) {
                            htmlUntilBlank = true
                        } else if i + 1 < n && isTableStart(md, text, lines[i + 1]) {
                            tableHeader = i
                            tableSplittable = !orderedOpen && !htmlUntilBlank
                        } else if firstNonSpace(md, text) == U.openBracket, let label = definitionLabel(md, text) {
                            if definitions[label] == nil { definitions[label] = i }
                        }
                    }
                    prevBlank = false
                }
                charsBefore[i + 1] = charsBefore[i] + lines[i].length + 1
                rowsBefore[i + 1] = rowsBefore[i] + (row[i] ? 1 : 0)
            }
            openAtEnd = fence != nil || htmlEnd != nil
        }

        func fits(_ from: Int, _ to: Int, _ budget: Budget) -> Bool {
            charsBefore[to] - charsBefore[from] <= budget.maxChars && to - from <= budget.maxLines
                && rowsBefore[to] - rowsBefore[from] <= budget.maxTableRows
        }

        private func lineText(_ index: Int) -> Units { Units(md[lines[index].start..<lines[index].end]) }

        /// Text that reopens the construct cut before line `at`: the fence's own opening line, or the table header +
        /// delimiter.
        func prefixFor(_ at: Int) -> Units {
            switch splitKind[at] {
            case splitFence: return lineText(splitRef[at]) + [U.newline]
            case splitTable: return lineText(splitRef[at]) + [U.newline] + lineText(splitRef[at] + 1) + [U.newline]
            default: return []
            }
        }

        func prefixLines(_ at: Int) -> Int {
            switch splitKind[at] {
            case splitFence: 1
            case splitTable: 2
            default: 0
            }
        }

        /// Text that closes the construct cut before line `at` (fenced code only, with the indent of its opening).
        func suffixFor(_ at: Int) -> Units {
            guard splitKind[at] == splitFence, let fence = openingFence(md, lines[splitRef[at]]) else { return [] }
            return [U.newline] + fence.closing
        }
    }

    /// Lines `[from, to)` of the source, then `prefix` / `suffix` added around them.
    private struct Piece {
        let from: Int
        let to: Int
        let prefix: Units
        let suffix: Units
        let oversized: Bool
    }

    /// The detail's chunks of `md`: runs of whole blocks within `budget` (characters, lines, table rows), cut as
    /// described above. Each rendered chunk carries the reference definitions it uses (8 KB at most). Linear in the
    /// length of `md`.
    public static func chunks(_ md: String, budget: Budget = chunkBudget) -> [Chunk] {
        if md.isEmpty { return [] }
        let s = Units(md.utf16)
        if s.allSatisfy(isWhitespace) { return [Chunk(markdown: md, asSource: false)] }
        let outline = Outline(s, splitLines(s))
        var pieces: [Piece] = []
        let starts = (0..<outline.n).filter { $0 == 0 || outline.cut[$0] }
        func segmentEnd(_ index: Int) -> Int { index + 1 < starts.count ? starts[index + 1] : outline.n }
        var index = 0
        while index < starts.count {
            let a = starts[index]
            if !outline.fits(a, segmentEnd(index), budget) {
                resplit(outline, a, segmentEnd(index), budget, &pieces)
                index += 1
                continue
            }
            var e = index + 1
            while e < starts.count && outline.fits(a, segmentEnd(e), budget) { e += 1 }
            // Stopped by the budget (not by an oversized block): rather cut just before a heading in the second half.
            if e < starts.count && !outline.heading[starts[e]] && outline.fits(starts[e], segmentEnd(e), budget) {
                let total = outline.charsBefore[segmentEnd(e - 1)] - outline.charsBefore[a]
                var k = e - 1
                while k >= index + 1 {
                    if (outline.charsBefore[starts[k]] - outline.charsBefore[a]) * 2 < total { break }
                    if outline.heading[starts[k]] {
                        e = k
                        break
                    }
                    k -= 1
                }
            }
            pieces.append(Piece(from: a, to: segmentEnd(e - 1), prefix: [], suffix: [], oversized: false))
            index = e
        }
        return pieces.map { toChunk(outline, $0) }
    }

    /// Cuts the oversized block of lines `[a, b)` at its split points: each piece as long as possible within `budget`,
    /// the reopening prefix and closing suffix included. A stretch without any split point stays whole (marked
    /// oversized).
    private static func resplit(_ o: Outline, _ a: Int, _ b: Int, _ budget: Budget, _ out: inout [Piece]) {
        var start = a
        var prefix: Units = []
        var chars = 0
        var lineCount = 0
        var rows = 0
        var lastOption = -1
        var over = false

        func begin(_ at: Int) {
            start = at
            prefix = at == a ? [] : o.prefixFor(at)
            chars = prefix.count
            lineCount = at == a ? 0 : o.prefixLines(at)
            rows = 0
            lastOption = -1
            over = false
        }
        func emit(_ to: Int, _ oversized: Bool) {
            out.append(Piece(from: start, to: to, prefix: prefix, suffix: to < b ? o.suffixFor(to) : [], oversized: oversized))
        }

        begin(a)
        var j = a
        while j < b {
            if j > start && o.splitKind[j] != splitNone {
                if over {
                    emit(j, true)
                    begin(j)
                    continue
                }
                lastOption = j
            }
            // Room kept for the fence that would close this piece right after line j.
            let closing = (j + 1 < b && o.splitKind[j + 1] == splitFence) ? o.suffixFor(j + 1).count : 0
            let row = o.row[j] ? 1 : 0
            let fits = chars + o.lines[j].length + 1 + closing <= budget.maxChars
                && lineCount + 1 + (closing > 0 ? 1 : 0) <= budget.maxLines && rows + row <= budget.maxTableRows
            if !over && !fits {
                if lastOption > start {
                    emit(lastOption, false)
                    let at = lastOption
                    begin(at)
                    j = at
                    continue
                }
                over = true
            }
            chars += o.lines[j].length + 1
            lineCount += 1
            rows += row
            j += 1
        }
        emit(b, over)
    }

    private static func toChunk(_ o: Outline, _ piece: Piece) -> Chunk {
        let start = o.lines[piece.from].start
        let end = o.lines[piece.to - 1].end
        if piece.oversized && end - start > unsplittableSourceChars {
            return Chunk(
                markdown: string(o.md[start..<end]), asSource: true, sourceStart: start, sourceEnd: end, prefixLength: 0,
                bodyLength: end - start, oversized: true
            )
        }
        var body = Units()
        body.reserveCapacity(piece.prefix.count + end - start + piece.suffix.count)
        body.append(contentsOf: piece.prefix)
        body.append(contentsOf: o.md[start..<end])
        body.append(contentsOf: piece.suffix)
        let bodyLength = body.count
        // Definitions appended after an unclosed fence would show as code.
        let definitions = (piece.to == o.n && o.openAtEnd) ? [] : usedDefinitions(o, body, piece.from, piece.to)
        if !definitions.isEmpty {
            body.append(U.newline)
            body.append(U.newline)
            appendJoined(definitions, to: &body)
        }
        return Chunk(
            markdown: string(body), asSource: false, sourceStart: start, sourceEnd: end, prefixLength: piece.prefix.count,
            bodyLength: bodyLength, oversized: piece.oversized
        )
    }

    /// Reference definitions the chunk `body` (lines `[from, to)`) uses without defining them: the first definition of
    /// each label, in order of first use, within `freeDefinitionsCap` characters. Labels are read from the brackets of
    /// the body (linear).
    private static func usedDefinitions(_ o: Outline, _ body: Units, _ from: Int, _ to: Int) -> [Units] {
        if o.definitions.isEmpty { return [] }
        var seen = Set<Units>()
        var result: [Units] = []
        var used = 0
        var i = 0
        let n = body.count
        while i < n {
            if body[i] != U.openBracket {
                i += 1
                continue
            }
            var close = -1
            var k = i + 1
            let limit = min(n, i + 1 + 999)
            while k < limit {
                let c = body[k]
                if c == U.backslash {
                    k += 2
                    continue
                }
                if c == U.openBracket { break }
                if c == U.closeBracket {
                    close = k
                    break
                }
                k += 1
            }
            if close < 0 {
                i += 1
                continue
            }
            let label = normalizeLabel(body, i + 1, close)
            i = close
            if label.isEmpty || !seen.insert(label).inserted { continue }
            guard let line = o.definitions[label] else { continue }
            if line >= from && line < to { continue }
            let text = trimmed(o.md, o.lines[line])
            if used + text.length + 1 > freeDefinitionsCap { continue }
            used += text.length + 1
            result.append(Units(o.md[text.start..<text.end]))
        }
        return result
    }

    /// Budget of one piece of a text shown literally in the detail.
    public static let sourceChunkLines = 100
    public static let sourceChunkChars = 4_000

    /// Pieces of a text shown literally (plain text, Markdown source, revealed secret), each its own text view: whole
    /// lines, at most `maxLines` lines and `maxChars` characters; a longer line is cut after a space (else on a safe
    /// boundary, never inside a surrogate pair or an emoji cluster). A piece of blank lines only joins its neighbour.
    /// Rejoined with their line separators, the pieces are the text.
    public static func sourceChunks(_ text: String, maxLines: Int = sourceChunkLines, maxChars: Int = sourceChunkChars) -> [String] {
        if text.isEmpty { return [] }
        let s = Units(text.utf16)
        return sourceChunkRanges(s, maxLines, maxChars).map { string(s[$0.start..<$0.end]) }
    }

    /// `sourceChunks` as `[start, end)` ranges of `text` (UTF-16 offsets).
    static func sourceChunkRanges(_ text: String, _ maxLines: Int, _ maxChars: Int) -> [Line] {
        sourceChunkRanges(Units(text.utf16), maxLines, maxChars)
    }

    private static func sourceChunkRanges(_ text: Units, _ maxLines: Int, _ maxChars: Int) -> [Line] {
        if text.isEmpty { return [] }
        let lineCap = max(1, maxLines)
        let charCap = max(2, maxChars)
        var ranges: [(start: Int, end: Int)] = []
        var pieceStart = -1
        var pieceEnd = 0
        var usedLines = 0
        var usedChars = 0
        for line in splitLines(text) {
            var pos = line.start
            while true {
                let remaining = line.end - pos
                if pieceStart >= 0 && (usedLines + 1 > lineCap || usedChars + remaining + 1 > charCap) {
                    ranges.append((pieceStart, pieceEnd))
                    pieceStart = -1
                }
                if pieceStart < 0 {
                    pieceStart = pos
                    usedLines = 0
                    usedChars = 0
                }
                if remaining + 1 <= charCap {
                    usedLines += 1
                    usedChars += remaining + 1
                    pieceEnd = line.end
                    break
                }
                // Longer than a whole piece: its beginning makes a piece of its own.
                let cut = cutInside(text, pos, line.end, charCap - 1)
                ranges.append((pieceStart, cut))
                pieceStart = -1
                pos = cut
            }
        }
        if pieceStart >= 0 { ranges.append((pieceStart, pieceEnd)) }
        // Blank pieces join a neighbour (no view for empty lines only).
        func isBlankRange(_ range: (start: Int, end: Int)) -> Bool {
            var i = range.start
            while i < range.end {
                if !isWhitespace(text[i]) { return false }
                i += 1
            }
            return true
        }
        var merged: [(start: Int, end: Int)] = []
        merged.reserveCapacity(ranges.count)
        for range in ranges {
            if isBlankRange(range) && !merged.isEmpty {
                merged[merged.count - 1].end = range.end
            } else if merged.count == 1 && isBlankRange(merged[0]) {
                merged[0].end = range.end
            } else {
                merged.append(range)
            }
        }
        return merged.map { Line(start: $0.start, end: $0.end) }
    }

    /// End of a piece taken from `pos` in a line ending at `end`, at most `room` (>= 1) characters, never empty.
    private static func cutInside(_ s: Units, _ pos: Int, _ end: Int, _ room: Int) -> Int {
        let limit = pos + room
        if limit >= end { return end }
        var j = limit - 1
        while j > pos + room / 2 {
            if s[j] == U.space || s[j] == U.tab { return j + 1 }
            j -= 1
        }
        let back = safeBoundary(s, limit, floor: pos)
        if back > pos { return back }
        // A cluster longer than the room: keep it whole.
        var p = limit
        while p < end && !isBoundary(s, p) { p += 1 }
        return p
    }
}
