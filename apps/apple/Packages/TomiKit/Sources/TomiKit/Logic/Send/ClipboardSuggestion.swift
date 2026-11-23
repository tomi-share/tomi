import Foundation

// The send screen offers what was just copied, without typing it: it shows in the place of the content, is used
// with one gesture, and goes away as soon as something else is typed. Only a copy made less than a minute ago is
// offered (an older one is no longer what the person is about to send).
//
// What each platform can know differs:
// - macOS reads its clipboard freely and Tomi runs all the time: the app sees when the clipboard changes and reads
//   what it holds, so the suggestion shows the content itself.
// - iOS tells neither when a copy was made nor, without asking the person ("Autoriser le collage ?"), what it is:
//   only that the clipboard changed and what kind of thing it holds. The suggestion names the kind, and the minute
//   runs from the moment Tomi sees the change (when it comes to the front).

/// What the send screen offers from the clipboard.
public enum ClipboardSuggestion: Hashable, Sendable {
    /// A text or a link, with what the screen shows of it (`ClipboardSuggestions.preview`).
    case text(preview: String, isLink: Bool)
    /// Copied files, by name.
    case files(names: [String])
    case image
    /// Marked concealed by the app that copied it (password managers), or it looks like a password: offered, never
    /// shown.
    case hidden
    /// Only the kind is known (iOS): the content is read when the person takes the suggestion.
    case unread(Kind)

    public enum Kind: Hashable, Sendable {
        case text, link, image
    }

    /// The content is known to the app: "Envoyer" can take the suggestion as it is. Not on iOS, where the content is
    /// only read (and shown) once the person asked for it.
    public var isRead: Bool {
        if case .unread = self { false } else { true }
    }
}

public enum ClipboardSuggestions {
    /// A copy older than this is no longer offered.
    public static let maxAge: TimeInterval = 60
    /// What the screen shows of a copied text: its first line, cut.
    public static let maxPreview = 120

    /// What is offered for a copied text; nil for an empty one. A text that looks like a password is offered without
    /// being shown (`SecretDetector`, the rule that turns "Masquer le contenu" on at a paste).
    public static func forText(_ text: String, concealed: Bool) -> ClipboardSuggestion? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if concealed || SecretDetector.looksLikeSecret(trimmed) { return .hidden }
        // Several lines are not "a password" as a whole, but the line shown may be one.
        let shown = preview(trimmed)
        let firstLine = shown.hasSuffix("…") ? String(shown.dropLast()) : shown
        if SecretDetector.looksLikeSecret(firstLine) { return .hidden }
        return .text(preview: shown, isLink: isLink(trimmed))
    }

    /// The first line that says something, at most `maxPreview` characters, with "…" when something is left out.
    public static func preview(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let lines = trimmed.split(omittingEmptySubsequences: true, whereSeparator: \.isNewline)
        guard let first = lines.first else { return "" }
        let line = first.trimmingCharacters(in: .whitespaces)
        let cut = line.count > maxPreview
        let shown = cut ? String(line.prefix(maxPreview)).trimmingCharacters(in: .whitespaces) : line
        return cut || lines.count > 1 ? shown + "…" : shown
    }

    /// One word that is an http(s) link.
    static func isLink(_ text: String) -> Bool {
        guard !text.contains(where: \.isWhitespace), let url = URL(string: text), let scheme = url.scheme?.lowercased() else { return false }
        return (scheme == "http" || scheme == "https") && url.host?.isEmpty == false
    }

    /// An empty draft only: a suggestion never competes with what the person already wrote, attached or sent.
    public static func canOffer(title: String, text: String, attachments: Int, sending: Bool, justSent: Bool) -> Bool {
        title.isBlank && text.isBlank && attachments == 0 && !sending && !justSent
    }
}

/// When the clipboard last changed, as far as the app saw it. The app gives it the change count of the clipboard
/// whenever it looks (macOS: every second; iOS: when it comes to the front and when the send screen opens).
public struct ClipboardClock: Sendable {
    private var lastCount: Int?
    private var changedAt: Date?

    public init() {}

    /// `count`: the change count of the clipboard now. The first look only takes note: a clipboard filled before the
    /// app looked has no known age, unless `firstLookCounts` (iOS: what was copied before opening Tomi is what the
    /// person comes to send).
    public mutating func observe(count: Int, now: Date, firstLookCounts: Bool = false) {
        defer { lastCount = count }
        guard let lastCount else {
            if firstLookCounts { changedAt = now }
            return
        }
        if count != lastCount { changedAt = now }
    }

    /// The app itself just wrote to the clipboard (a copy made in Tomi): not something to offer back.
    public mutating func ignore(count: Int) {
        lastCount = count
        changedAt = nil
    }

    /// A change the app does not want to offer (made while it was in front): noted, without a date. A copy offered
    /// before it stays out too: it is no longer what the clipboard holds.
    public mutating func ignoreChange(count: Int) {
        if count != lastCount { changedAt = nil }
        lastCount = count
    }

    /// The suggestion was taken or dismissed: not offered again until the clipboard changes.
    public mutating func consume() {
        changedAt = nil
    }

    /// The clipboard changed, and less than a minute ago.
    public func isFresh(now: Date) -> Bool {
        guard let changedAt else { return false }
        let age = now.timeIntervalSince(changedAt)
        return age >= 0 && age < ClipboardSuggestions.maxAge
    }
}
