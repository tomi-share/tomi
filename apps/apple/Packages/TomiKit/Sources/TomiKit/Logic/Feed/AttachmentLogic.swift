import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/AttachmentLogic.kt.
// Name lengths and cut positions are UTF-16 code units, as Kotlin's `String` counts them (a name is handled as an array of
// `UInt16`, so that the same cases give the same cuts).

/// Pure rules of the attachment tiles of a card (feed and detail). No UI: unit-tested. The views are in
/// `Tomi/Features/Feed/AttachmentViews.swift`.
public enum AttachmentLogic {
    /// Attachments shown by a feed card before "N autres fichiers" (the detail shows them all).
    public static let maxVisible = 3

    /// Longest extension that is still shown as such in the badge of a tile ("PDF", "WEBP"); "drawio" gets an icon instead.
    public static let maxBadgeExtension = 4

    /// An ellipsis that fits is worth more than a long "extension" made of the end of a sentence.
    private static let maxExtensionLength = 10

    /// Result of `splitExtension`: `radical` + `extension` (the dot included, e.g. ".pdf") put back together give the name.
    public struct NameParts: Equatable, Sendable {
        public let radical: String
        public let `extension`: String

        public init(radical: String, extension fileExtension: String) {
            self.radical = radical
            self.extension = fileExtension
        }
    }

    /// Splits `Courrier banque.pdf` into "Courrier banque" and ".pdf". No extension (empty `extension`) for a name without a
    /// dot, with a leading dot only (".gitignore"), ending with a dot, or whose "extension" has a space or is too long to be one.
    public static func splitExtension(_ name: String) -> NameParts {
        let units = Array(name.utf16)
        guard let dot = units.lastIndex(of: 0x2E), dot > 0 else { return NameParts(radical: name, extension: "") }
        let tail = String(decoding: units[dot...], as: UTF16.self)
        let tailLength = units.count - dot
        if tailLength == 1 || tailLength > maxExtensionLength || tail.unicodeScalars.contains(where: isWhitespace) {
            return NameParts(radical: name, extension: "")
        }
        return NameParts(radical: String(decoding: units[..<dot], as: UTF16.self), extension: tail)
    }

    /// The upper-case extension shown in the badge of a tile ("PDF", "KT"), or nil when there is none or it is longer than 4 letters.
    public static func badgeExtension(_ name: String) -> String? {
        var fileExtension = splitExtension(name).extension
        if fileExtension.hasPrefix(".") { fileExtension.removeFirst() }
        if fileExtension.isEmpty || fileExtension.utf16.count > maxBadgeExtension { return nil }
        if !fileExtension.unicodeScalars.allSatisfy(isLetterOrDigit) { return nil }
        return fileExtension.uppercased()
    }

    /// The name cut BEFORE its extension so that it fits `maxLines` lines: "Courrier banque fonds de rési….pdf". `fits` says
    /// whether a candidate text fits (in the views: the measure of the text in the available width). It is assumed monotonic
    /// (a shorter radical fits whenever a longer one does), which is what lets the search be a dichotomy. A name that already
    /// fits is returned as is. Without an extension, the end of the name is cut ("Courrier banque rési…").
    public static func fitFileName(_ name: String, fits: (String) -> Bool) -> String {
        if fits(name) { return name }
        let parts = splitExtension(name)
        let units = Array(parts.radical.utf16)
        func candidate(_ length: Int) -> String {
            let head = String(decoding: units[..<safeCut(units, length)], as: UTF16.self)
            return trimEnd(head) + "…" + parts.extension
        }
        // The longest radical length that still fits; 0 (just "…" + extension) is the floor, even if that does not fit either.
        var low = 0
        var high = units.count - 1
        while low < high {
            let middle = (low + high + 1) / 2
            if fits(candidate(middle)) { low = middle } else { high = middle - 1 }
        }
        return candidate(low)
    }

    /// Never cuts between the two halves of a surrogate pair (an emoji in a file name).
    private static func safeCut(_ units: [UInt16], _ length: Int) -> Int {
        length >= 1 && length < units.count && UTF16.isLeadSurrogate(units[length - 1]) ? length - 1 : length
    }

    /// Kotlin's `Char.isWhitespace()`: the space, line and paragraph separators, and the control characters 9 to 13 and 28 to 31.
    private static func isWhitespace(_ scalar: Unicode.Scalar) -> Bool {
        if (9...13).contains(scalar.value) || (0x1C...0x1F).contains(scalar.value) { return true }
        switch scalar.properties.generalCategory {
        case .spaceSeparator, .lineSeparator, .paragraphSeparator: return true
        default: return false
        }
    }

    /// Kotlin's `Char.isLetterOrDigit()`: a letter or a decimal digit.
    private static func isLetterOrDigit(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.properties.generalCategory {
        case .uppercaseLetter, .lowercaseLetter, .titlecaseLetter, .modifierLetter, .otherLetter, .decimalNumber: return true
        default: return false
        }
    }

    /// Kotlin's `trimEnd()`: without the trailing whitespace.
    private static func trimEnd(_ text: String) -> String {
        var scalars = Array(text.unicodeScalars)
        while let last = scalars.last, isWhitespace(last) { scalars.removeLast() }
        return String(String.UnicodeScalarView(scalars))
    }

    /// How one attachment is drawn when it is not in a mosaic.
    public enum Look: Sendable {
        /// A tile: type badge, name, size, action.
        case tile
        /// An image frame: the thumbnail and its caption in a single object.
        case frame
    }

    public struct Entry: Equatable, Sendable {
        public let index: Int
        public let look: Look

        public init(index: Int, look: Look) {
            self.index = index
            self.look = look
        }
    }

    public enum Plan: Equatable, Sendable {
        /// Only images (2 or more), none of them without a thumbnail: `shown` cells (2 or 3), "+`more`" on the last when `more` > 0.
        case mosaic(shown: Int, more: Int)
        /// Tiles and frames in order, then "`more` autres fichiers" when `more` > 0.
        case items(entries: [Entry], more: Int)
    }

    /// What a card shows for its attachments.
    /// - Detail (`expanded`): everything, in order; an image with a thumbnail is a frame, anything else a tile.
    /// - Feed: `maxVisible` attachments at most. Images only (2 or more, with thumbnails): a mosaic. A single image with a
    ///   thumbnail: a frame. Everything else: tiles (an image becomes a tile with a mini-thumbnail).
    /// - `showThumbnails` false (a revealed hidden item): nothing but tiles, no thumbnail anywhere.
    public static func plan(_ attachments: [Attachment], expanded: Bool, showThumbnails: Bool) -> Plan {
        func framed(_ attachment: Attachment) -> Bool {
            showThumbnails && attachment.isImage && attachment.thumbnail != nil
        }
        if expanded {
            let entries = attachments.enumerated().map { Entry(index: $0.offset, look: framed($0.element) ? .frame : .tile) }
            return .items(entries: entries, more: 0)
        }
        let visible = Array(attachments.prefix(maxVisible))
        let more = attachments.count - visible.count
        if attachments.count >= 2 && visible.allSatisfy(framed) && attachments.allSatisfy(\.isImage) {
            return .mosaic(shown: visible.count, more: more)
        }
        let single = attachments.count == 1 && framed(attachments[0])
        let entries = visible.enumerated().map { Entry(index: $0.offset, look: single && framed($0.element) ? .frame : .tile) }
        return .items(entries: entries, more: more)
    }

    /// "4 fichiers · 4,9 Mo" is shown above 2 attachments or more of the detail; "5 images · …" when they are all images.
    public static func showsSummary(_ attachments: [Attachment]) -> Bool { attachments.count >= 2 }

    public static func isAllImages(_ attachments: [Attachment]) -> Bool {
        !attachments.isEmpty && attachments.allSatisfy(\.isImage)
    }

    public static func totalSize(_ attachments: [Attachment]) -> Int64 {
        attachments.reduce(0) { $0 + $1.size }
    }
}
