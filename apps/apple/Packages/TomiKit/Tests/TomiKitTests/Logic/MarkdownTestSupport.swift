import XCTest
@testable import TomiKit

// What the ports of MarkdownBlocksTest.kt and MarkdownChunksTest.kt share. Kotlin strings are UTF-16: every length,
// offset and comparison of those tests is done here on UTF-16 code units (a Swift `String` compares by canonical
// equivalence and counts grapheme clusters: "\r\n" is one `Character`, and "x\r\n".hasSuffix("\n") is false).

/// `java.util.Random`: the fuzz tests then run on the very documents of the Android tests.
struct JavaRandom {
    private var seed: UInt64
    private static let mask: UInt64 = (1 << 48) - 1

    init(_ seed: Int64) {
        self.seed = (UInt64(bitPattern: seed) ^ 0x5DEECE66D) & Self.mask
    }

    private mutating func next(_ bits: UInt64) -> Int32 {
        seed = (seed &* 0x5DEECE66D &+ 0xB) & Self.mask
        return Int32(truncatingIfNeeded: seed >> (48 - bits))
    }

    mutating func nextInt(_ bound: Int) -> Int {
        let bound = Int32(bound)
        var r = next(31)
        let m = bound - 1
        if bound & m == 0 {
            r = Int32(truncatingIfNeeded: (Int64(bound) * Int64(r)) >> 31)
        } else {
            var u = r
            r = u % bound
            while u &- r &+ m < 0 {
                u = next(31)
                r = u % bound
            }
        }
        return Int(r)
    }

    mutating func nextBoolean() -> Bool { next(1) != 0 }
}

/// Kotlin's `Char.isWhitespace()`.
func kIsWhitespace(_ u: UInt16) -> Bool {
    switch u {
    case 0x09...0x0D, 0x1C...0x20, 0xA0, 0x1680, 0x2000...0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000: true
    default: false
    }
}

extension String {
    /// Kotlin `length`.
    var kLength: Int { utf16.count }

    func kStartsWith(_ prefix: String) -> Bool { utf16.starts(with: prefix.utf16) }

    func kEndsWith(_ suffix: String) -> Bool {
        let a = Array(utf16)
        let b = Array(suffix.utf16)
        return a.count >= b.count && a[(a.count - b.count)...].elementsEqual(b)
    }

    /// Offset of the first `needle` at or after `from`, or nil.
    func kIndex(of needle: String, from: Int = 0) -> Int? {
        let a = Array(utf16)
        let b = Array(needle.utf16)
        if b.isEmpty { return from }
        guard a.count >= b.count else { return nil }
        var i = from
        while i <= a.count - b.count {
            if a[i] == b[0] && a[i..<(i + b.count)].elementsEqual(b) { return i }
            i += 1
        }
        return nil
    }

    func kContains(_ needle: String) -> Bool { kIndex(of: needle) != nil }

    /// Kotlin `substring(from, to)`.
    func kSub(_ from: Int, _ to: Int? = nil) -> String {
        let a = Array(utf16)
        return String(decoding: a[from..<(to ?? a.count)], as: UTF16.self)
    }

    /// Kotlin `split(separator)`: every piece, the empty ones included.
    func kSplit(_ separator: String) -> [String] {
        let a = Array(utf16)
        let b = Array(separator.utf16)
        var pieces: [String] = []
        var start = 0
        var i = 0
        while i + b.count <= a.count {
            if a[i..<(i + b.count)].elementsEqual(b) {
                pieces.append(String(decoding: a[start..<i], as: UTF16.self))
                i += b.count
                start = i
            } else {
                i += 1
            }
        }
        pieces.append(String(decoding: a[start...], as: UTF16.self))
        return pieces
    }

    /// Kotlin `substringBefore(delimiter)`: the whole string when the delimiter is not in it.
    func kSubstringBefore(_ delimiter: String) -> String {
        guard let at = kIndex(of: delimiter) else { return self }
        return kSub(0, at)
    }

    func kReplacing(_ target: String, with replacement: String) -> String { kSplit(target).joined(separator: replacement) }

    func kTrimEnd() -> String {
        var a = Array(utf16)
        while let last = a.last, kIsWhitespace(last) { a.removeLast() }
        return String(decoding: a, as: UTF16.self)
    }

    /// Kotlin `removeSuffix`.
    func kRemoveSuffix(_ suffix: String) -> String { kEndsWith(suffix) ? kSub(0, kLength - suffix.kLength) : self }

    /// Kotlin `isBlank()`.
    var kIsBlank: Bool { utf16.allSatisfy(kIsWhitespace) }

    static func * (text: String, count: Int) -> String { String(repeating: text, count: count) }
}

/// Kotlin `assertEquals` on two strings: the same code units.
func assertSame(_ expected: String, _ actual: String, _ message: String = "", file: StaticString = #filePath, line: UInt = #line) {
    XCTAssertTrue(expected.utf16.elementsEqual(actual.utf16), message.isEmpty ? "the two texts differ" : message, file: file, line: line)
}

/// A cut inside a surrogate pair leaves a lone surrogate, which decodes to U+FFFD (the test texts hold none).
func assertWellFormed(_ text: String, file: StaticString = #filePath, line: UInt = #line) {
    XCTAssertFalse(text.unicodeScalars.contains("\u{FFFD}"), "a surrogate pair was cut", file: file, line: line)
}

func numberedLines(_ count: Int, _ prefix: String = "line") -> String {
    (0..<count).map { "\(prefix) \($0)" }.joined(separator: "\n")
}

func elapsedMilliseconds(_ body: () -> Void) -> Int {
    let start = DispatchTime.now().uptimeNanoseconds
    body()
    return Int((DispatchTime.now().uptimeNanoseconds - start) / 1_000_000)
}
