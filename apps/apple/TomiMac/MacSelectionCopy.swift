import AppKit
import CoreGraphics

/// The global shortcut sends what is selected in the app in front (docs/macos-app.md, 2.2): Tomi asks
/// that app to copy its selection (Command-C, as the person would), then opens the send screen, which offers what
/// was just copied (text, files, an image; a password stays hidden). The selection therefore replaces what the
/// clipboard held: it is what the person asked for with the shortcut.
///
/// Sending a key to another app needs the permission of macOS (Privacy & Security, Accessibility). Without it the
/// shortcut opens Tomi and copies nothing.
@MainActor
enum MacSelectionCopy {
    private static let askedKey = "shortcut_copy_access_asked"

    /// A copy is being asked (the shortcut pressed twice in a row asks once).
    private(set) static var copying = false

    /// macOS lets Tomi send a key to the app in front.
    static var allowed: Bool { CGPreflightPostEventAccess() }

    /// Asks macOS for the permission (its own dialog, shown once by the system).
    static func requestAccess() {
        UserDefaults.standard.set(true, forKey: askedKey)
        CGRequestPostEventAccess()
    }

    /// The first press without the permission asks for it; the next ones do not insist (the settings say how).
    static func requestAccessOnce() {
        guard !UserDefaults.standard.bool(forKey: askedKey) else { return }
        requestAccess()
    }

    /// Asks the app in front to copy its selection, and waits for the clipboard to change. True when it did (false:
    /// nothing was selected, the app copies nothing, or the permission is missing).
    static func copySelection(wait: Duration = .milliseconds(600)) async -> Bool {
        guard allowed, !copying else { return false }
        copying = true
        defer { copying = false }
        // The keys of the shortcut are still down: an app that reads the keyboard itself would see them with the C.
        // Wait for them to be let go, a short while.
        let held: CGEventFlags = [.maskControl, .maskShift, .maskAlternate, .maskCommand]
        for _ in 0..<15 where !CGEventSource.flagsState(.combinedSessionState).isDisjoint(with: held) {
            try? await Task.sleep(for: .milliseconds(20))
        }
        let board = NSPasteboard.general
        let before = board.changeCount
        // A source of its own: the keys of the shortcut still held down (Control, Shift) are not mixed in.
        let source = CGEventSource(stateID: .privateState)
        let keyC: CGKeyCode = 8
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: keyC, keyDown: down) else { return false }
            event.flags = .maskCommand
            event.post(tap: .cgAnnotatedSessionEventTap)
        }
        let step = Duration.milliseconds(40)
        var waited = Duration.zero
        while waited < wait {
            try? await Task.sleep(for: step)
            waited += step
            if board.changeCount != before { return true }
        }
        return false
    }
}
