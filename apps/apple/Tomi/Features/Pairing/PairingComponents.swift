import SwiftUI
import TomiDesign
import TomiKit
import UIKit
import UserNotifications

/// The end of the entry, shared by "Ton espace est prêt" and "Appareil ajouté" (Android `PairedContent`): a green check,
/// the title and the sentence; notifications are asked here, right after joining, never at first launch.
struct PairedContent: View {
    let title: LocalizedStringKey
    let message: String
    let onFinished: () -> Void
    @State private var status: UNAuthorizationStatus?

    var body: some View {
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s3, alignment: .center) {
                TomiIcon.delivered.image
                    .font(.system(size: 64, weight: .light))
                    .foregroundStyle(TomiColor.success)
                    .accessibilityHidden(true)
                Text(title).tomiText(.title).foregroundStyle(TomiColor.text).multilineTextAlignment(.center)
                    .accessibilityAddTraits(.isHeader)
                Text(verbatim: message)
                    .tomiText(.body)
                    .foregroundStyle(TomiColor.textMuted)
                    .multilineTextAlignment(.center)
                if status == .notDetermined {
                    Text("pair_notifications_rationale")
                        .tomiText(.body)
                        .foregroundStyle(TomiColor.text)
                        .multilineTextAlignment(.center)
                        .padding(.top, TomiSpace.s3)
                }
            }
            .padding(.horizontal, TomiSpace.s5)
            BottomActions {
                if status == .notDetermined {
                    Button {
                        Task {
                            _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound])
                            onFinished()
                        }
                    } label: {
                        Label(L10n.text("pair_notifications_enable"), systemImage: "bell")
                    }
                    .buttonStyle(.tomiPrimary)
                    Button(L10n.text("action_later"), action: onFinished).buttonStyle(.tomiText)
                } else if status != nil {
                    Button(L10n.text("action_continue"), action: onFinished).buttonStyle(.tomiPrimary)
                }
            }
        }
        .task { status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus }
    }
}

/// "Valable encore 1:48" until `deadline` (monotonic clock), refreshed every second, hidden at 0: an indication, never a
/// cut-off.
struct ValidityText: View {
    let deadline: ContinuousClock.Instant?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { _ in
            let now = ContinuousClock.now
            if let deadline, deadline > now {
                let left = now.duration(to: deadline).components
                let ms = left.seconds * 1000 + left.attoseconds / 1_000_000_000_000_000
                Text(verbatim: L10n.text("pair_verify_validity", Countdown.format(remainingMs: ms)))
                    .tomiText(.caption)
                    .monospacedDigit()
                    .foregroundStyle(TomiColor.textMuted)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
            }
        }
    }
}

/// Six digits in boxes, a gap after the third: mono 28 semibold, tabular digits (charter). Read in two groups of three.
struct VerificationDigits: View {
    let code: String

    var body: some View {
        HStack(spacing: TomiSpace.s2) {
            ForEach(Array(code.enumerated()), id: \.offset) { index, digit in
                if index == 3 { Spacer().frame(width: TomiSpace.s2) }
                Text(String(digit))
                    .tomiText(.pairingCode)
                    .monospacedDigit()
                    .foregroundStyle(TomiColor.text)
                    .frame(width: 44, height: 60)
                    .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
                    .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border))
            }
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(L10n.text("pair_verify_code_description", PairingInput.groupedVerificationCode(code))))
    }
}

/// Buttons kept at the bottom, above the home indicator.
struct BottomActions<Content: View>: View {
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(spacing: TomiSpace.s1) { content() }
            .padding(TomiSpace.screen)
            .frame(maxWidth: TomiLayout.readableWidth)
            .frame(maxWidth: .infinity)
    }
}

/// The camera preview of a scan, `height` points high, rounded. The camera is asked only when the user touches the
/// scan button (`CameraAccess.request`).
struct ScannerPreview: View {
    let height: CGFloat
    let description: LocalizedStringKey
    let onScanned: (String) -> Void

    var body: some View {
        QRScanner(onScanned: onScanned)
            .frame(height: height)
            .clipShape(RoundedRectangle(cornerRadius: TomiRadius.large))
            .accessibilityElement()
            .accessibilityLabel(Text(description))
    }
}

/// What to say when the camera cannot be used: none on this device (simulator), or refused (with a way to the
/// settings, iOS only).
struct CameraProblem: View {
    let denied: Bool
    let deniedMessage: LocalizedStringKey
    let unavailableMessage: LocalizedStringKey
    @Environment(\.openURL) private var openURL

    var body: some View {
        if !QRScanner.isAvailable {
            Text(unavailableMessage).tomiText(.body).foregroundStyle(TomiColor.textMuted)
        } else if denied {
            VStack(alignment: .leading, spacing: TomiSpace.s1) {
                Text(deniedMessage).tomiText(.body).foregroundStyle(TomiColor.error)
                Button(L10n.text("ios_pair_camera_settings")) {
                    if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                }
                .buttonStyle(.tomiText)
            }
        }
    }
}

/// A back button for the screens that decide themselves what "back" does (close the scanner first, leave a pairing).
struct EntryBackButton: ToolbarContent {
    let action: () -> Void

    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Button(action: action) { TomiIcon.back.image }
                .accessibilityLabel(Text("action_back"))
        }
    }
}

// MARK: Relay

/// The "Relais" field of the pairing and creation screens (spec/pairing-v1.md 4.4 rule 6, 10.3; Android `RelayField`).
///
/// - `required` (the device has no relay): the field is open, nothing folds it.
/// - otherwise it is folded under "Autre relais" (open as soon as something is typed or a link filled it), and the relay
///   the device already has is named above it.
///
/// What is typed is only a candidate: the core checks it, the next screen shows it in full, and the core keeps it only once
/// the pairing or the creation worked.
struct RelayField: View {
    let text: String
    let onChange: (String) -> Void
    let required: Bool
    let deviceAuthority: String?
    let disabled: Bool
    @State private var expanded = false
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s2) {
            if !required, let deviceAuthority {
                Text(verbatim: L10n.text("relay_current", deviceAuthority))
                    .tomiText(.body)
                    .foregroundStyle(TomiColor.textMuted)
            }
            if required || expanded || !text.isBlank {
                VStack(alignment: .leading, spacing: TomiSpace.s1) {
                    Text("relay_field_label").tomiText(.label).foregroundStyle(TomiColor.text)
                    TextField("", text: Binding(get: { text }, set: { onChange($0) }), prompt: Text(verbatim: "relay.example.org"))
                        .tomiText(.mono)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .textContentType(nil)
                        .submitLabel(.done)
                        .focused($focused)
                        .disabled(disabled)
                        .tomiFieldBox(focused: focused)
                        .accessibilityLabel(Text("relay_field_label"))
                    Text("relay_field_hint").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                }
            } else {
                Button(L10n.text("relay_other_button")) { expanded = true }
                    .buttonStyle(.tomiText)
                    .disabled(disabled)
            }
        }
    }
}

/// The relay in full, with the sentence that asks the user to check it (Android `RelayCheckCard`). A relay read from a QR
/// code, a link or a field is not trusted: whoever showed it chose it. `label` and `hint` are text keys; `label` takes the
/// authority (`host[:port]`).
struct RelayCheckCard: View {
    let authority: String
    let label: String
    let hint: String

    var body: some View {
        TomiCard {
            VStack(alignment: .leading, spacing: TomiSpace.s1) {
                Text(verbatim: L10n.text(label, authority))
                    .tomiMono(size: 15)
                    .foregroundStyle(TomiColor.text)
                    .fixedSize(horizontal: false, vertical: true)
                Text(verbatim: L10n.text(hint))
                    .tomiText(.caption)
                    .foregroundStyle(TomiColor.textMuted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }
}
