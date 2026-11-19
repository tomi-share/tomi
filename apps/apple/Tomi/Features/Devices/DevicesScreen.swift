import SwiftUI
import TomiDesign
import TomiKit

/// The devices of the space (Android `DevicesScreen`): rename any of them (40 characters), remove another one from
/// the space behind Face ID / Touch ID only (the space key is renewed).
struct DevicesScreen: View {
    let core: any TomiCore
    let gate: BiometricGate
    let feedback: FeedbackCenter
    /// "Ajouter un appareil": the invitation screen.
    var onAddDevice: () -> Void = {}
    @State private var renaming: Device?
    @State private var newName = ""
    @State private var revoking: Device?
    /// A revocation runs: it renews the space key at the relay, which takes several seconds.
    @State private var busy = false

    static let maxName = 40

    var body: some View {
        let devices = core.state.devices
        ZStack(alignment: .bottom) {
            TomiColor.bg.ignoresSafeArea()
            if core.state.loaded && devices.isEmpty {
                VStack(spacing: TomiSpace.s3) {
                    TomiIcon.devices.image.font(.system(size: 44, weight: .light)).foregroundStyle(TomiColor.textMuted)
                    Text("devices_empty").tomiText(.body).foregroundStyle(TomiColor.textMuted).multilineTextAlignment(.center)
                }
                .padding(TomiSpace.s6)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            List {
                if core.state.rotationStalled {
                    RotationStalledBanner()
                        .listRowBackground(Color.clear)
                }
                if busy {
                    // Linear bar (charter 6.3), nothing near the symbol.
                    VStack(alignment: .leading, spacing: TomiSpace.s2) {
                        TomiLinearProgress()
                        Text("devices_revoking").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                    }
                    .listRowBackground(Color.clear)
                    .accessibilityElement(children: .combine)
                }
                ForEach(devices) { device in
                    row(device)
                        .listRowBackground(TomiColor.surface)
                }
            }
            .scrollContentBackground(.hidden)
            .frame(maxWidth: TomiLayout.readableWidth)
            .frame(maxWidth: .infinity)
            FeedbackHost(center: feedback, bottomPadding: TomiSpace.s4)
        }
        .navigationTitle(Text("devices_title"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: addDevice) { Image(systemName: "person.badge.plus") }
                    .accessibilityLabel(Text("devices_add"))
            }
        }
        .alert(L10n.text("devices_rename"), isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("", text: Binding(get: { newName }, set: { newName = String($0.prefix(Self.maxName)) }))
            Button(L10n.text("action_cancel"), role: .cancel) { renaming = nil }
            Button(L10n.text("action_save")) { rename() }
                .disabled(newName.isBlank)
        }
        .alert(
            revoking.map { L10n.text("devices_revoke_title", $0.name) } ?? "",
            isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } })
        ) {
            Button(L10n.text("action_cancel"), role: .cancel) { revoking = nil }
            Button(L10n.text("devices_revoke"), role: .destructive) { revoke() }
        } message: {
            Text("devices_revoke_message")
        }
    }

    private func row(_ device: Device) -> some View {
        HStack(spacing: TomiSpace.s3) {
            (device.platform == .android || device.platform == .ios ? TomiIcon.phone : TomiIcon.computer).image
                .foregroundStyle(TomiColor.text)
                .frame(width: 24)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: device.name).tomiText(.body).foregroundStyle(TomiColor.text)
                Text(verbatim: subtitle(device)).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
            }
            Spacer(minLength: TomiSpace.s2)
            Button {
                newName = device.name
                renaming = device
            } label: {
                Image(systemName: "pencil").frame(width: 44, height: 44)
            }
            .buttonStyle(.borderless)
            .foregroundStyle(TomiColor.text)
            .disabled(busy)
            .accessibilityLabel(Text("devices_rename"))
            if !device.isThisDevice {
                Button { revoking = device } label: {
                    Image(systemName: "minus.circle").frame(width: 44, height: 44)
                }
                .buttonStyle(.borderless)
                .foregroundStyle(TomiColor.error)
                .disabled(busy)
                .accessibilityLabel(Text("devices_revoke"))
            }
        }
        .accessibilityElement(children: .contain)
    }

    /// "Cet appareil · iOS · il y a 5 min".
    private func subtitle(_ device: Device) -> String {
        var parts: [String] = []
        if device.isThisDevice { parts.append(L10n.text("devices_this_device")) }
        parts.append(L10n.text(device.platform.labelKey))
        if let last = device.lastActivity {
            let formatter = RelativeDateTimeFormatter()
            formatter.locale = Locale(identifier: "fr_FR")
            formatter.unitsStyle = .full
            parts.append(formatter.localizedString(for: last, relativeTo: Date()))
        }
        return parts.joined(separator: " · ")
    }

    private func rename() {
        guard let device = renaming else { return }
        let name = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        renaming = nil
        guard !name.isEmpty, name != device.name else { return }
        Task {
            do {
                try await core.renameDevice(id: device.id, name: name)
            } catch {
                feedback.show(L10n.text("devices_rename_failed"))
            }
        }
    }

    /// "Ajouter un appareil": Face ID / Touch ID or the passcode first, every time (never the 2-minute window), before
    /// the invitation code is made: whoever holds the unlocked phone cannot add a device to the space alone.
    private func addDevice() {
        Task {
            switch await gate.authenticateFresh(reason: L10n.text("ios_devices_add_reason")) {
            case .success:
                onAddDevice()
            case .unavailable:
                feedback.show(L10n.text("ios_devices_add_unavailable"))
            case .cancelled, .failed:
                break
            }
        }
    }

    private func revoke() {
        guard let device = revoking, !busy else { return }
        revoking = nil
        Task {
            // Destructive: Face ID / Touch ID every time, never the passcode, never remembered.
            switch await gate.authenticateStrong(reason: L10n.text("ios_devices_revoke_reason")) {
            case .success:
                guard !busy else { return }
                busy = true
                AccessibilityNotification.Announcement(L10n.text("devices_revoking")).post()
                defer { busy = false }
                do {
                    try await core.revokeDevice(id: device.id)
                } catch {
                    feedback.show(L10n.text("devices_revoke_failed"))
                }
            case .unavailable:
                feedback.show(L10n.text("devices_revoke_unavailable"))
            case .cancelled, .failed:
                break
            }
        }
    }
}

extension DevicePlatform {
    var labelKey: String {
        switch self {
        case .android: "platform_android"
        case .windows: "platform_windows"
        case .ios: "ios_platform_ios"
        case .macos: "ios_platform_macos"
        case .cli: "ios_platform_cli"
        case .other: "platform_other"
        }
    }
}
