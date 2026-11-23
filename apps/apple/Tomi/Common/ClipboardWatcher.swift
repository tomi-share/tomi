import Foundation
import TomiKit
#if os(iOS)
import UIKit
#else
import AppKit
#endif

/// What the send screen may offer from the clipboard (`ClipboardSuggestion`): something copied less than a minute
/// ago, outside Tomi. Offered, never typed: the person takes it with one gesture, or writes something else.
///
/// macOS: Tomi runs all the time and looks at the change count of the clipboard every second (a number: nothing of
/// what was copied is read), so it knows when a copy was made; the content is read only when the send screen opens
/// on a fresh copy. iOS gives neither the time of a copy nor, without asking the person, its content: Tomi notes
/// that the clipboard changed when it comes to the front, and offers the kind of thing it holds.
@MainActor
final class ClipboardWatcher {
    static let shared = ClipboardWatcher()

    private var clock = ClipboardClock()
    /// The change count of the clipboard when the suggestion on screen was made.
    private var offered: Int?
    private let now: () -> Date = Date.init
    #if os(macOS)
    private var timer: Timer?
    #endif

    private var changeCount: Int {
        #if os(iOS)
        UIPasteboard.general.changeCount
        #else
        NSPasteboard.general.changeCount
        #endif
    }

    /// Takes note of the state of the clipboard now.
    func look() {
        #if os(macOS)
        // A copy made while Tomi is in front is made in Tomi (a text selected in a card, an image): never offered
        // back. So is whatever lands on the clipboard meanwhile (Universal Clipboard): the person did not come from
        // it. The shortcut copies in another app, while Tomi is not in front.
        if NSApp.isActive {
            clock.ignoreChange(count: changeCount)
            return
        }
        #endif
        #if os(iOS)
        // What was copied before opening Tomi is what the person comes to send: the first look counts.
        clock.observe(count: changeCount, now: now(), firstLookCounts: true)
        #else
        clock.observe(count: changeCount, now: now())
        #endif
    }

    /// Tomi just wrote to the clipboard itself (a copy of an item): not something to offer back.
    func noticeOwnCopy() {
        clock.ignore(count: changeCount)
    }

    /// The suggestion was taken or dismissed: not offered again until something else is copied.
    func consume() {
        look()
        clock.consume()
    }

    /// What to offer now: nil unless the clipboard changed less than a minute ago and holds something to send.
    func suggestion() -> ClipboardSuggestion? {
        let suggestion = read()
        offered = suggestion == nil ? nil : changeCount
        return suggestion
    }

    /// The suggestion on screen is still what the clipboard holds, and still less than a minute old: taking it gives
    /// what was shown, nothing copied since (another device, a clipboard manager).
    func stillOffers() -> Bool {
        offered != nil && offered == changeCount && clock.isFresh(now: now())
    }

    private func read() -> ClipboardSuggestion? {
        look()
        guard clock.isFresh(now: now()) else { return nil }
        #if os(iOS)
        let board = UIPasteboard.general
        // Never Tomi's own secret copy. The checks below read no content (no "Autoriser le collage ?").
        if board.contains(pasteboardTypes: [SensitiveClipboard.tomiSecretType]) { return nil }
        if board.hasURLs { return .unread(.link) }
        if board.hasStrings { return .unread(.text) }
        return nil
        #else
        let board = NSPasteboard.general
        if board.types?.contains(NSPasteboard.PasteboardType(SensitiveClipboard.tomiSecretType)) == true { return nil }
        // Marked concealed by the app that copied it (password managers): offered, and not even read.
        if MacSendInput.holdsConcealed(board) { return .hidden }
        let files = (board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? [])
            .filter(\.isFileURL)
        if !files.isEmpty { return .files(names: files.map(\.lastPathComponent)) }
        // An image with no text beside it: its types say so, it is not decoded to be offered.
        if MacSendInput.holdsFilesOrImage(board) { return .image }
        guard let text = board.string(forType: .string) ?? board.string(forType: .html).map(HtmlText.toPlain) else { return nil }
        return ClipboardSuggestions.forText(text, concealed: false)
        #endif
    }

    #if os(macOS)
    /// Starts looking at the clipboard every second, for as long as the app runs.
    func startWatching() {
        guard timer == nil else { return }
        look()
        let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.look() }
        }
        // A menu open, a window being resized: the look still happens.
        RunLoop.main.add(timer, forMode: .common)
        timer.tolerance = 0.3
        self.timer = timer
    }
    #endif
}

extension ClipboardSuggestion {
    /// What the content field shows in the place of the content, in the colour of a placeholder.
    var ghostText: String {
        switch self {
        case let .text(preview, _): preview
        case let .files(names):
            L10n.text("ios_clip_files", names.prefix(3).joined(separator: ", ") + (names.count > 3 ? "…" : ""))
        case .image: L10n.text("ios_clip_image")
        case .hidden: L10n.text("ios_clip_hidden")
        case .unread(.link): L10n.text("ios_clip_link")
        case .unread: L10n.text("ios_clip_text")
        }
    }
}
