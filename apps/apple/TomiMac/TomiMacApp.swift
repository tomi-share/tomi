import SwiftUI
import TomiDesign
import TomiKit

/// Tomi for the Mac (docs/macos-app.md): a menu bar app. The icon beside the clock opens a popover with
/// the feed and the send screen; the full window opens on demand; the app keeps running with every window closed.
/// The screens and the models are the ones of the iOS app; `MacAppDelegate` owns the surfaces.
@main
struct TomiMacApp: App {
    @NSApplicationDelegateAdaptor(MacAppDelegate.self) private var delegate

    init() {
        TomiFonts.register()
    }

    var body: some Scene {
        // SwiftUI needs a scene; every window of Tomi (popover, full window, settings) is an AppKit window shown by
        // the delegate, because they must open from the menu bar icon, with no SwiftUI scene on screen. This scene
        // only carries the commands: "Settings…" of the app menu (Command-comma) opens the delegate's settings.
        Settings {
            EmptyView()
        }
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button(L10n.text("macos_menu_settings")) { delegate.showSettings() }
                    .keyboardShortcut(",", modifiers: .command)
            }
            CommandGroup(replacing: .newItem) {
                Button(L10n.text("macos_menu_new_send")) { delegate.perform(.newSend) }
                    .keyboardShortcut("n", modifiers: .command)
                Button(L10n.text("macos_menu_window")) { delegate.perform(.window) }
                    .keyboardShortcut("0", modifiers: .command)
            }
            CommandGroup(after: .textEditing) {
                Button(L10n.text("action_search")) { delegate.perform(.search) }
                    .keyboardShortcut("f", modifiers: .command)
            }
            CommandMenu(L10n.text("macos_menu_feed")) {
                ForEach(Array(FeedFilter.allCases.prefix(9).enumerated()), id: \.offset) { index, filter in
                    Button(L10n.text(filter.labelKey)) { delegate.perform(.filter(filter)) }
                        .keyboardShortcut(KeyEquivalent(Character("\(index + 1)")), modifiers: .command)
                }
                Divider()
                Button(L10n.text("devices_title")) { delegate.perform(.devices) }
            }
        }
    }
}

extension View {
    /// What the screens shared with iOS expect and macOS does differently by default: a button or a menu without a
    /// style of its own is drawn as its label (macOS would give it a bezel, and a menu its arrow), a text field has
    /// no frame of its own (the screens draw theirs), a toggle is a switch.
    func macScreenDefaults() -> some View {
        menuStyle(.button)
            .buttonStyle(.plain)
            .menuIndicator(.hidden)
            .textFieldStyle(.plain)
            .toggleStyle(.switch)
    }
}
