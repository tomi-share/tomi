import AppKit
import SwiftUI
import UserNotifications

/// The settings window of the Mac app (Command-comma), with the controls of macOS.
struct MacSettingsView: View {
    let shortcut: MacGlobalShortcut?
    let sendShortcut: MacGlobalShortcut?
    @State private var opensAtLogin = MacLoginItem.isEnabled
    @State private var copyAllowed = MacSelectionCopy.allowed
    @State private var notifications: UNAuthorizationStatus?
    @State private var downloads = MacDownloads.displayName
    @State private var downloadsChosen = MacDownloads.hasChosenFolder

    var body: some View {
        Form {
            Section {
                ShortcutRow(shortcut: shortcut, title: "macos_settings_shortcut", hint: "macos_settings_shortcut_hint")
                ShortcutRow(
                    shortcut: sendShortcut, title: "macos_settings_send_shortcut",
                    hint: copyAllowed ? "macos_settings_send_shortcut_hint" : "macos_settings_copy_selection_denied"
                )
                if !copyAllowed {
                    Button(L10n.text("macos_settings_copy_selection_open")) {
                        MacSelectionCopy.requestAccess()
                        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
                            NSWorkspace.shared.open(url)
                        }
                    }
                }
                LabeledContent {
                    switch notifications {
                    case .notDetermined?:
                        Button(L10n.text("pair_notifications_enable")) {
                            Task {
                                _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound])
                                await readNotifications()
                            }
                        }
                    case .denied?:
                        Button(L10n.text("macos_settings_notifications_open")) {
                            if let url = URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension") {
                                NSWorkspace.shared.open(url)
                            }
                        }
                    case nil:
                        EmptyView()
                    default:
                        Text("macos_settings_notifications_on").foregroundStyle(.secondary)
                    }
                } label: {
                    Text("macos_settings_notifications")
                    Text(notifications == .denied ? "macos_settings_notifications_denied" : "macos_settings_notifications_hint")
                }
                Toggle(isOn: Binding(get: { opensAtLogin }, set: { wanted in
                    MacLoginItem.setEnabled(wanted)
                    // What the system kept, whatever was asked.
                    opensAtLogin = MacLoginItem.isEnabled
                })) {
                    Text("macos_settings_login")
                    Text("macos_settings_login_hint")
                }
            }
            Section {
                LabeledContent {
                    HStack(spacing: 8) {
                        Text(verbatim: downloads).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                        Button(L10n.text("macos_settings_downloads_choose")) { chooseDownloads() }
                        if downloadsChosen {
                            Button(L10n.text("macos_settings_downloads_default")) {
                                MacDownloads.reset()
                                refreshDownloads()
                            }
                        }
                    }
                } label: {
                    Text("macos_settings_downloads")
                    Text("macos_settings_downloads_hint")
                }
            }
            Section {
                LabeledContent {
                    Text(verbatim: AppContainer.appVersion)
                } label: {
                    Text("macos_settings_version")
                }
                Button(L10n.text("macos_menu_quit")) { NSApp.terminate(nil) }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520)
        .fixedSize(horizontal: false, vertical: true)
        .onAppear {
            opensAtLogin = MacLoginItem.isEnabled
            copyAllowed = MacSelectionCopy.allowed
        }
        .task { await readNotifications() }
        // Back from the system settings: the state may have changed.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            copyAllowed = MacSelectionCopy.allowed
            Task { await readNotifications() }
        }
    }

    /// The open panel on folders: the sandbox gives the app lasting access to the one picked there.
    private func chooseDownloads() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let folder = panel.url else { return }
        _ = MacDownloads.choose(folder)
        refreshDownloads()
    }

    private func refreshDownloads() {
        downloads = MacDownloads.displayName
        downloadsChosen = MacDownloads.hasChosenFolder
    }

    private func readNotifications() async {
        notifications = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }
}

/// One global shortcut in the settings: its keys, and "Modifier…" to record new ones.
private struct ShortcutRow: View {
    let shortcut: MacGlobalShortcut?
    let title: LocalizedStringKey
    let hint: LocalizedStringKey
    @State private var text = ""
    @State private var recording = false
    @State private var refused = false
    @State private var monitor: Any?

    var body: some View {
        LabeledContent {
            HStack(spacing: 8) {
                Text(verbatim: recording ? L10n.text("macos_settings_shortcut_recording") : text)
                    .monospaced()
                    .foregroundStyle(recording ? .secondary : .primary)
                Button(L10n.text(recording ? "action_cancel" : "macos_settings_shortcut_change")) {
                    if recording { stopRecording() } else { startRecording() }
                }
            }
        } label: {
            Text(title)
            Text(refused ? "macos_settings_shortcut_refused" : hint)
                .foregroundStyle(refused ? Color.red : Color.secondary)
        }
        .onAppear { refresh() }
        .onDisappear { stopRecording() }
    }

    private func refresh() {
        text = shortcut?.isRegistered == true ? shortcut?.current.display ?? "" : "—"
    }

    /// The next key pressed with Command, Control or Option becomes the shortcut; Escape gives up.
    private func startRecording() {
        refused = false
        recording = true
        shortcut?.suspend()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            if event.keyCode == 53 {
                stopRecording()
                return nil
            }
            guard let pressed = MacShortcut(event: event) else { return nil }
            if shortcut?.set(pressed) != true { refused = true }
            stopRecording()
            return nil
        }
    }

    private func stopRecording() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        recording = false
        shortcut?.resume()
        refresh()
    }
}
