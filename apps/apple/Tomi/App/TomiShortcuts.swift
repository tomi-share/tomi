import AppIntents
import SwiftUI

/// "Envoyer avec Tomi" in Shortcuts, Spotlight and the Action button (Android: the quick-settings tile): opens the send
/// screen.
struct OpenSendIntent: AppIntent {
    static let title: LocalizedStringResource = "shortcut_long_label"
    static let openAppWhenRun = true

    @MainActor
    func perform() async throws -> some IntentResult {
        PendingShortcut.openSend = true
        NotificationCenter.default.post(name: PendingShortcut.didChange, object: nil)
        return .result()
    }
}

struct TomiShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: OpenSendIntent(),
            phrases: ["Envoyer avec \(.applicationName)", "Envoyer un élément avec \(.applicationName)"],
            shortTitle: "tile_label",
            systemImageName: "paperplane"
        )
    }
}

/// The shortcut ran before the feed was there (a cold start): taken when it shows.
@MainActor
enum PendingShortcut {
    static var openSend = false
    static let didChange = Notification.Name("eu.shaffe.tomi.shortcut")
}
