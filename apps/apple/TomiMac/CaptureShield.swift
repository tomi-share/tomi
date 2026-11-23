import AppKit
import SwiftUI

// The Mac counterpart of the capture-protected layer of iOS (`Tomi/Common/CaptureShield.swift`). macOS cannot leave
// one view out of captures, but it can leave a whole window out (`NSWindow.sharingType = .none`): the window is kept
// out of screenshots, recordings and screen sharing for as long as it shows something protected (what a hidden item
// contains once shown, a pairing code), and only then (docs/macos-app.md, A6).

extension View {
    /// Keeps the window out of captures while this view is on screen and `active`.
    func captureProtected(_ active: Bool = true) -> some View {
        background(CaptureProtector(active: active).frame(width: 0, height: 0).accessibilityHidden(true))
    }
}

private struct CaptureProtector: NSViewRepresentable {
    let active: Bool

    func makeNSView(context: Context) -> ProtectorView {
        let view = ProtectorView()
        view.active = active
        return view
    }

    func updateNSView(_ view: ProtectorView, context: Context) {
        view.active = active
    }
}

/// Counts itself in the window it is in while active, and out when it leaves it or stops being active.
private final class ProtectorView: NSView {
    var active = false { didSet { if active != oldValue { update() } } }
    private weak var counted: NSWindow?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        update()
    }

    private func update() {
        let target = active ? window : nil
        guard counted !== target else { return }
        if let counted { WindowCaptureGuard.release(counted) }
        if let target { WindowCaptureGuard.retain(target) }
        counted = target
    }
}

/// How many protected views each window shows; the window is out of captures while it shows at least one.
@MainActor
enum WindowCaptureGuard {
    /// Weak keys: a window closed with protected views still in it takes its count away with it.
    private static let counts = NSMapTable<NSWindow, NSNumber>.weakToStrongObjects()

    /// Developer builds only (Debug, Dev): captures stay possible (screenshots of the app being developed) unless
    /// the app is launched with `-TomiProtectCaptures YES`. The code does not exist in a Release build, where the
    /// protection is always on.
    private static var enabled: Bool {
        #if DEBUG || TOMI_DEV_TOOLS
        UserDefaults.standard.bool(forKey: "TomiProtectCaptures")
        #else
        true
        #endif
    }

    static func retain(_ window: NSWindow) {
        let count = (counts.object(forKey: window)?.intValue ?? 0) + 1
        counts.setObject(NSNumber(value: count), forKey: window)
        if enabled { window.sharingType = .none }
    }

    static func release(_ window: NSWindow) {
        let left = (counts.object(forKey: window)?.intValue ?? 1) - 1
        if left > 0 {
            counts.setObject(NSNumber(value: left), forKey: window)
        } else {
            counts.removeObject(forKey: window)
            // The default of a window (Tomi never sets another value).
            window.sharingType = .readOnly
        }
    }
}
