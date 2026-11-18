import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/share/SharePayloadParser.kt.
// Android `content://` streams (SharedStream, StreamPolicy, PackageManager checks) become `SharedFile`: a file the
// share extension already copied into the app group, with the metadata it knows.

/// A file received through the share extension, already copied to `url`.
public struct SharedFile: Hashable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var url: URL
    public var name: String
    public var mime: String
    public var size: Int64

    public init(url: URL, name: String, mime: String, size: Int64) {
        self.url = url
        self.name = name
        self.mime = mime
        self.size = size
    }

    public var description: String { "SharedFile(<redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// Limits on shared files (Android `StreamPolicy`).
public enum ShareLimits {
    public static let maxFileBytes: Int64 = 25 * 1024 * 1024
    public static let maxItemBytes: Int64 = 100 * 1024 * 1024
    public static let maxFiles = 20

    /// True if one file is over `maxFileBytes`, or the files together are over `maxItemBytes`.
    public static func exceeds(_ files: [SharedFile]) -> Bool {
        files.contains { $0.size > maxFileBytes } || files.reduce(Int64(0)) { $0 + max($1.size, 0) } > maxItemBytes
    }
}

/// Raw values of what the share extension received, already extracted from the system types.
public struct ShareInput: Hashable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var subject: String?
    public var text: String?
    public var htmlText: String?
    public var files: [SharedFile]
    /// Identifier of the sharing app, when known.
    public var sourcePackage: String?
    /// True if the source marked the content as sensitive.
    public var markedSensitive: Bool

    public init(
        subject: String? = nil, text: String? = nil, htmlText: String? = nil, files: [SharedFile] = [],
        sourcePackage: String? = nil, markedSensitive: Bool = false
    ) {
        self.subject = subject
        self.text = text
        self.htmlText = htmlText
        self.files = files
        self.sourcePackage = sourcePackage
        self.markedSensitive = markedSensitive
    }

    /// What another app shared is content: never printed.
    public var description: String { "ShareInput(markedSensitive=\(markedSensitive), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// One composite item built from a share: the whole share always becomes a single item.
public struct ParsedShare: Hashable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var title: String?
    public var text: RichText?
    public var url: String?
    public var files: [SharedFile]
    public var looksSecret: Bool

    public init(title: String?, text: RichText?, url: String?, files: [SharedFile], looksSecret: Bool) {
        self.title = title
        self.text = text
        self.url = url
        self.files = files
        self.looksSecret = looksSecret
    }

    public var isEmpty: Bool { title == nil && text == nil && url == nil && files.isEmpty }

    public var description: String { "ParsedShare(looksSecret=\(looksSecret), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

public enum SharePayloadParser {
    public static let maxTitle = 300
    public static let maxText = 100_000
    public static let maxHTML = 500_000
    private static let maxTitleFromText = 150

    public static func parse(_ input: ShareInput) -> ParsedShare {
        let subject = TextSanitizer.clean(input.subject, maxLength: maxTitle)
        let html = input.htmlText.flatMap { $0.isBlank ? nil : String($0.prefix(maxHTML)) }
        let rawText = TextSanitizer.clean(input.text, maxLength: maxText)
            ?? html.flatMap { TextSanitizer.clean(HtmlText.toPlain($0), maxLength: maxText) }

        var title = subject
        var url: String?
        var body = rawText

        // Safari, Chrome and most apps send "Title URL" as text: split it into title + url.
        if html == nil, let rawText, let parts = LinkExtractor.extract(rawText) {
            url = parts.url
            body = nil
            if let leftover = parts.leftover {
                if let current = title {
                    if !sameText(current, leftover) { body = leftover }
                } else if leftover.count <= maxTitleFromText && !leftover.contains("\n") {
                    title = leftover
                } else {
                    body = leftover
                }
            }
        }

        let text = body.map { plain in
            RichText(
                plain: plain,
                html: html,
                markdown: html == nil && MarkdownDetector.looksLikeMarkdown(plain) ? plain : nil
            )
        }

        let files = Array(input.files.prefix(ShareLimits.maxFiles))

        let secret = input.markedSensitive
            || SecretDetector.isPasswordManager(input.sourcePackage)
            || (url == nil && files.isEmpty && body != nil && SecretDetector.looksLikeSecret(body))

        return ParsedShare(title: title, text: text, url: url, files: files, looksSecret: secret)
    }

    private static func sameText(_ a: String, _ b: String) -> Bool {
        a.trimmingCharacters(in: .whitespacesAndNewlines).caseInsensitiveCompare(b.trimmingCharacters(in: .whitespacesAndNewlines)) == .orderedSame
    }
}

public enum OutgoingDraftFactory {
    /// Builds the draft handed to the core. The expiry is validated against the secret flag and the attachments: a choice that is
    /// not allowed (e.g. "after first read" for plain content, or for an item with a file) falls back to the default, which is no expiry.
    public static func create(
        id: String, parsed: ParsedShare, attachments: [DraftAttachment], secret: Bool, expiry: ExpiryChoice, now: Date
    ) -> OutgoingDraft {
        let resolved = ExpiryPolicy.resolve(expiry, secret: secret, hasAttachment: !attachments.isEmpty, now: now)
        return OutgoingDraft(
            id: id, title: parsed.title, text: parsed.text, url: parsed.url, attachments: attachments,
            secret: secret, expiresAt: resolved.expiresAt, readPolicy: resolved.readPolicy
        )
    }
}
