import AppKit

/// The menu bar icon of Tomi: a click opens or closes the popover, a right click (or Control-click, or Option-click)
/// shows the menu.
/// A dot on the icon tells that something received is not read yet.
@MainActor
final class MacStatusItem: NSObject {
    private let item: NSStatusItem
    private let onClick: (NSStatusBarButton) -> Void
    private let onOpenWindow: () -> Void
    private let onSettings: () -> Void
    private var unread = false

    var button: NSStatusBarButton? { item.button }

    init(onClick: @escaping (NSStatusBarButton) -> Void, onOpenWindow: @escaping () -> Void, onSettings: @escaping () -> Void) {
        self.onClick = onClick
        self.onOpenWindow = onOpenWindow
        self.onSettings = onSettings
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        super.init()
        item.button?.image = Self.icon(unread: false)
        item.button?.setAccessibilityLabel(L10n.text("app_name"))
        item.button?.target = self
        item.button?.action = #selector(clicked(_:))
        item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
    }

    func setUnread(_ value: Bool) {
        guard value != unread else { return }
        unread = value
        item.button?.image = Self.icon(unread: value)
        item.button?.setAccessibilityLabel(L10n.text(value ? "macos_status_unread" : "app_name"))
    }

    @objc private func clicked(_ sender: NSStatusBarButton) {
        let event = NSApp.currentEvent
        if event?.type == .rightMouseUp || event?.modifierFlags.isDisjoint(with: [.control, .option]) == false {
            showMenu()
        } else {
            onClick(sender)
        }
    }

    private func showMenu() {
        let menu = NSMenu()
        menu.addItem(entry("macos_menu_open", #selector(openPopover)))
        menu.addItem(entry("macos_menu_window", #selector(openWindow)))
        menu.addItem(entry("macos_menu_settings", #selector(openSettings), key: ","))
        menu.addItem(.separator())
        menu.addItem(entry("macos_menu_quit", #selector(quit), key: "q"))
        // Shown as the menu of the item for this click only: a left click stays the popover.
        item.menu = menu
        item.button?.performClick(nil)
        item.menu = nil
    }

    private func entry(_ key: String, _ action: Selector, key equivalent: String = "") -> NSMenuItem {
        let entry = NSMenuItem(title: L10n.text(key), action: action, keyEquivalent: equivalent)
        entry.target = self
        return entry
    }

    @objc private func openPopover() {
        if let button = item.button { onClick(button) }
    }

    @objc private func openWindow() { onOpenWindow() }
    @objc private func openSettings() { onSettings() }
    @objc private func quit() { NSApp.terminate(nil) }

    // MARK: Icon

    /// The symbol drawn for 16 px exactly (`design/tomi/logo-16.svg`, charte 2.4: never the big symbol scaled down),
    /// as rectangles of its pixel grid: x, y, width, height.
    private static let pixels: [(Int, Int, Int, Int)] = [
        (6, 2, 6, 1), (5, 3, 8, 1), (5, 4, 2, 1), (11, 4, 3, 1), (12, 5, 3, 1), (3, 6, 2, 1), (13, 6, 2, 1),
        (2, 7, 4, 1), (13, 7, 2, 1), (2, 8, 4, 1), (13, 8, 2, 1), (3, 9, 2, 1), (13, 9, 2, 1), (12, 10, 3, 1),
        (5, 11, 2, 1), (11, 11, 3, 1), (5, 12, 8, 1), (6, 13, 6, 1),
    ]

    /// A template image (the menu bar gives it its colour, light or dark). Unread: a dot in the top right corner, with
    /// a clear ring around it so that it stands out from the symbol.
    private static func icon(unread: Bool) -> NSImage {
        let image = NSImage(size: NSSize(width: 18, height: 16), flipped: true) { _ in
            NSColor.black.setFill()
            let symbol = NSBezierPath()
            for (x, y, width, height) in pixels {
                // The 16 px drawing, centred in the 18 pt of the item.
                symbol.appendRect(NSRect(x: x + 1, y: y, width: width, height: height))
            }
            guard unread else {
                symbol.fill()
                return true
            }
            let dot = NSRect(x: 12, y: 0, width: 6, height: 6)  // top right corner
            // The ring: the symbol is drawn everywhere but around the dot.
            NSGraphicsContext.saveGraphicsState()
            let outside = NSBezierPath(rect: NSRect(x: 0, y: 0, width: 18, height: 16))
            outside.append(NSBezierPath(ovalIn: dot.insetBy(dx: -1.5, dy: -1.5)))
            outside.windingRule = .evenOdd
            outside.addClip()
            symbol.fill()
            NSGraphicsContext.restoreGraphicsState()
            NSBezierPath(ovalIn: dot).fill()
            return true
        }
        image.isTemplate = true
        return image
    }
}
