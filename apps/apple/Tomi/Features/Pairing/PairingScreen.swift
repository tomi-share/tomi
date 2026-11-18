import SwiftUI
import TomiDesign
import TomiKit

/// "Rejoins ton espace" (Android `PairingScreen`, designs 01-pairing-v1-iter2 and 09-appairage-etats): QR scan or
/// typed invitation code, then the 6-digit check, then ONE yes / no question about the history, then the wait for the
/// other device, then "Appareil ajouté". Back leaves the pairing at every step after the input (it is cancelled); on
/// the code step it goes back to the scan step; while scanning it closes the scanner.
struct PairingScreen: View {
    @Bindable var model: PairingViewModel
    /// Name of this device once joined ("iPhone de Camille fait partie de ton espace").
    let localDeviceName: String
    let onFinished: () -> Void
    /// Debug builds: opens the demo core and pairs it in one go, then straight to the feed.
    var openDemo: (() async -> (any TomiCore)?)?
    var onDemoEntered: () -> Void = {}
    @Environment(\.dismiss) private var dismiss
    @State private var scanning = false

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            switch model.phase {
            case .done:
                PairedContent(title: "pair_done_title", message: L10n.text("pair_done_body", localDeviceName), onFinished: onFinished)
            case .verify: VerifyContent(model: model)
            case .history: HistoryContent(model: model)
            case .waiting: WaitingContent(model: model)
            case .input, .working:
                if model.enteringCode {
                    CodeEntryContent(model: model) {
                        model.enteringCode = false
                        scanning = true
                    }
                } else {
                    ScanContent(model: model, scanning: $scanning, openDemo: openDemo, onDemoEntered: onDemoEntered)
                }
            }
        }
        .navigationBarBackButtonHidden(true)
        .toolbar {
            if model.phase != .done { EntryBackButton(action: back) }
        }
        .sensitiveScreen()
        // The relay the device already has decides whether the "Relais" field is required.
        .task { await model.loadDeviceRelay() }
        .animation(.easeOut(duration: TomiMotion.medium), value: model.phase)
        .animation(.easeOut(duration: TomiMotion.medium), value: model.enteringCode)
    }

    private func back() {
        if model.phase != .input {
            model.leave()
        } else if scanning {
            scanning = false
        } else if model.enteringCode {
            model.enteringCode = false
        } else {
            dismiss()
        }
    }
}

// MARK: Input

/// The scan step: the path to follow on the other device, its illustration (or the camera), the two ways in.
private struct ScanContent: View {
    let model: PairingViewModel
    @Binding var scanning: Bool
    let openDemo: (() async -> (any TomiCore)?)?
    let onDemoEntered: () -> Void
    @State private var cameraDenied = false

    var body: some View {
        let working = model.phase == .working
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: TomiSpace.s4) {
                    Text("pair_title").tomiText(.title).foregroundStyle(TomiColor.text)
                        .accessibilityAddTraits(.isHeader)
                    PathToInvite()
                    LinkReceivedCard(model: model)
                    if scanning && !working {
                        ScannerPreview(height: 320, description: "pair_scan_preview_description") { value in
                            scanning = false
                            model.onQRScanned(value)
                        }
                    } else {
                        OtherDeviceIllustration()
                    }
                    Text("pair_scan_hint")
                        .tomiText(.caption)
                        .foregroundStyle(TomiColor.textMuted)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity)
                    CameraProblem(denied: cameraDenied, deniedMessage: "pair_camera_denied", unavailableMessage: "ios_pair_camera_unavailable")
                    PairingStatus(model: model)
                }
                .padding(TomiSpace.screen)
                .frame(maxWidth: TomiLayout.readableWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            BottomActions {
                Button(L10n.text("pair_scan_button")) {
                    Task {
                        let granted = await QRScanner.requestAccess()
                        cameraDenied = !granted
                        scanning = granted
                    }
                }
                .buttonStyle(.tomiPrimary)
                .disabled(working || !QRScanner.isAvailable)
                Button(L10n.text("pair_enter_code_button")) {
                    scanning = false
                    model.enteringCode = true
                }
                .buttonStyle(.tomiSecondary)
                .disabled(working)
                #if DEBUG
                if let openDemo {
                    Button(L10n.text("ios_debug_enter_demo")) {
                        Task { if await model.enterDemo(open: openDemo) { onDemoEntered() } }
                    }
                    .buttonStyle(.tomiText)
                    .disabled(working)
                }
                #endif
            }
        }
        .onChange(of: model.phase) { _, phase in if phase != .input { scanning = false } }
    }
}

/// "Sur ton autre appareil, ouvre Appareils › Ajouter un appareil", the path on one unbreakable line.
private struct PathToInvite: View {
    var body: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s1) {
            Text("pair_intro").tomiText(.body).foregroundStyle(TomiColor.textMuted)
            Text(verbatim: "\(L10n.text("devices_title"))\u{00A0}›\u{00A0}\(L10n.text("devices_add"))")
                .tomiText(.body)
                .fontWeight(.semibold)
                .foregroundStyle(TomiColor.text)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
        }
        .accessibilityElement(children: .combine)
    }
}

/// What the other device shows: "Ajouter un appareil", a QR code and an example code.
private struct OtherDeviceIllustration: View {
    var body: some View {
        VStack(spacing: TomiSpace.s2) {
            Text("pair_other_device_caption").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
            VStack(spacing: TomiSpace.s2) {
                Text("devices_add").tomiText(.label).foregroundStyle(TomiColor.text)
                TomiIcon.scanQR.image
                    .font(.system(size: 96, weight: .ultraLight))
                    .foregroundStyle(TomiColor.text)
                    .frame(width: 112, height: 112)
                Text(verbatim: "7-crayon-renard").tomiMono(size: 13).foregroundStyle(TomiColor.textMuted)
            }
            .padding(TomiSpace.s4)
            .frame(maxWidth: 240)
            .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.large))
            .overlay(RoundedRectangle(cornerRadius: TomiRadius.large).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width))
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text("pair_illustration_description"))
    }
}

/// The typed invitation code ("Saisis le code d'invitation"). The keyboard's key does not submit: "Rejoindre" does.
private struct CodeEntryContent: View {
    let model: PairingViewModel
    let onScanInstead: () -> Void
    @FocusState private var focused: Bool

    var body: some View {
        let working = model.phase == .working
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: TomiSpace.s4) {
                    Text("pair_code_title").tomiText(.title).foregroundStyle(TomiColor.text)
                        .accessibilityAddTraits(.isHeader)
                    Text("pair_code_intro").tomiText(.body).foregroundStyle(TomiColor.textMuted)
                    LinkReceivedCard(model: model)
                    VStack(alignment: .leading, spacing: TomiSpace.s1) {
                        Text("pair_code_label").tomiText(.label).foregroundStyle(TomiColor.text)
                        TextField("", text: Binding(get: { model.code }, set: { model.onCodeChange($0) }))
                            .tomiText(.mono)
                            .keyboardType(.asciiCapable)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .textContentType(nil)
                            .submitLabel(.done)
                            .focused($focused)
                            .disabled(working)
                            .tomiFieldBox(focused: focused)
                            .accessibilityLabel(Text("pair_code_label"))
                        Text("pair_code_hint").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                    }
                    // Required when the device has no relay; otherwise folded under "Autre relais".
                    RelayField(
                        text: model.relayText, onChange: { model.onRelayChange($0) },
                        required: model.relayFieldRequired, deviceAuthority: model.deviceRelayAuthority,
                        disabled: working
                    )
                    PairingStatus(model: model)
                }
                .padding(TomiSpace.screen)
                .frame(maxWidth: TomiLayout.readableWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .scrollDismissesKeyboard(.interactively)
            BottomActions {
                Button(L10n.text("pair_join")) { model.joinWithCode() }
                    .buttonStyle(.tomiPrimary)
                    .disabled(working || model.code.isBlank)
                Button(L10n.text("pair_scan_instead"), action: onScanInstead)
                    .buttonStyle(.tomiText)
                    .disabled(working)
            }
        }
        .onAppear { focused = true }
    }
}

/// A pairing link opened the app: offered, never followed without a touch.
private struct LinkReceivedCard: View {
    let model: PairingViewModel

    var body: some View {
        if model.pendingLink != nil {
            TomiCard {
                VStack(alignment: .leading, spacing: TomiSpace.s2) {
                    Text("pair_link_received").tomiText(.subtitle).foregroundStyle(TomiColor.text)
                    // The relay of the link is not trusted: read in full here, and again with the verification code.
                    if let authority = model.pendingLinkRelayAuthority {
                        Text(verbatim: L10n.text("pair_link_received_relay", authority))
                            .tomiMono(size: 15)
                            .foregroundStyle(TomiColor.text)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Text("pair_link_received_hint").tomiText(.body).foregroundStyle(TomiColor.textMuted)
                    Button(L10n.text("pair_join")) { model.joinWithPendingLink() }
                        .buttonStyle(.tomiPrimary)
                        .disabled(model.phase == .working)
                }
            }
        }
    }
}

/// The error in red, or the connection in progress with its way out.
private struct PairingStatus: View {
    let model: PairingViewModel

    var body: some View {
        if let error = model.error {
            Text(L10n.text(error)).tomiText(.body).foregroundStyle(TomiColor.error)
        }
        if model.phase == .working {
            VStack(alignment: .leading, spacing: TomiSpace.s2) {
                // Charter 6.3: progress is linear, never a spinner.
                TomiLinearProgress()
                Text("pair_working").tomiText(.body).foregroundStyle(TomiColor.textMuted)
                Button(L10n.text("action_cancel")) { model.leave() }.buttonStyle(.tomiText)
            }
        }
    }
}

// MARK: Verify

/// The same 6-digit code must show on both devices.
private struct VerifyContent: View {
    let model: PairingViewModel

    var body: some View {
        if let awaiting = model.awaiting {
            VStack(spacing: 0) {
                CenteredScroll(spacing: TomiSpace.s2) {
                    Text("pair_verify_title").tomiText(.title).foregroundStyle(TomiColor.text)
                        .accessibilityAddTraits(.isHeader)
                    Text("pair_verify_body").tomiText(.body).foregroundStyle(TomiColor.textMuted)
                    PeerRow(prompt: awaiting).padding(.top, TomiSpace.s4)
                    // The relay of this pairing, in full, next to the code (spec/pairing-v1.md 4.4 rule 7): it may come
                    // from a QR code or a field, so the user reads it and checks the code with the other device.
                    if !awaiting.relayAuthority.isEmpty {
                        RelayCheckCard(authority: awaiting.relayAuthority, label: "pair_verify_relay_label", hint: "pair_verify_relay_hint")
                            .padding(.top, TomiSpace.s3)
                    }
                    VerificationDigits(code: awaiting.verificationCode).padding(.top, TomiSpace.s5)
                    ValidityText(deadline: model.validUntil).padding(.top, TomiSpace.s2)
                }
                .padding(.horizontal, TomiSpace.screen)
                BottomActions {
                    Button(L10n.text("pair_verify_same")) { model.codesMatch() }
                        .buttonStyle(.tomiPrimary)
                        .disabled(model.busy)
                    Button(L10n.text("pair_verify_different")) { model.codesDiffer() }
                        .buttonStyle(.tomiText)
                }
            }
        }
    }
}

/// The other device: its name, then how many devices the space has (or its platform when unknown).
private struct PeerRow: View {
    let prompt: PairingPrompt

    var body: some View {
        HStack(spacing: TomiSpace.s3) {
            TomiIcon.computer.image
                .foregroundStyle(TomiColor.text)
                .frame(width: 40, height: 40)
                .background(TomiColor.surfaceAlt, in: Circle())
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: prompt.peer.name).tomiText(.subtitle).foregroundStyle(TomiColor.text)
                Text(verbatim: prompt.memberCount.map { L10n.plural("pair_verify_peer_members", $0) }
                    ?? L10n.text(prompt.peer.platform.labelKey))
                    .tomiText(.caption)
                    .foregroundStyle(TomiColor.textMuted)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: History

/// One yes / no choice, no box per kind of content; asked only when the space holds something.
private struct HistoryContent: View {
    @Bindable var model: PairingViewModel

    var body: some View {
        let count = model.awaiting?.historyItemCount ?? 0
        let bytes = model.awaiting?.historyBytes ?? 0
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s3) {
                Text("pair_history_title").tomiText(.title).foregroundStyle(TomiColor.text)
                    .accessibilityAddTraits(.isHeader)
                Text(L10n.plural("pair_history_body", count)).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                if bytes > 0 {
                    Text(L10n.text("pair_history_size", formatFileSize(bytes))).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                }
                VStack(spacing: TomiSpace.s3) {
                    HistoryChoice(selected: model.importHistory, title: "pair_history_yes", hint: "pair_history_yes_hint") {
                        model.importHistory = true
                    }
                    HistoryChoice(selected: !model.importHistory, title: "pair_history_no", hint: "pair_history_no_hint") {
                        model.importHistory = false
                    }
                }
                .padding(.top, TomiSpace.s2)
                Text("pair_history_footer").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
            }
            .padding(.horizontal, TomiSpace.screen)
            BottomActions {
                Button(L10n.text("action_continue")) { model.confirmHistory() }
                    .buttonStyle(.tomiPrimary)
                    .disabled(model.busy)
            }
        }
    }
}

private struct HistoryChoice: View {
    let selected: Bool
    let title: LocalizedStringKey
    let hint: LocalizedStringKey
    let onSelect: () -> Void

    var body: some View {
        Button(action: onSelect) {
            HStack(alignment: .top, spacing: TomiSpace.s2) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .font(.system(size: 20))
                    .foregroundStyle(selected ? TomiColor.brand : TomiColor.textMuted)
                VStack(alignment: .leading, spacing: TomiSpace.s1) {
                    Text(title).tomiText(.label).foregroundStyle(TomiColor.text)
                    Text(hint).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                }
                Spacer(minLength: 0)
            }
            .padding(TomiSpace.s3)
            .background(selected ? TomiColor.brandContainer : TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
            .overlay(
                RoundedRectangle(cornerRadius: TomiRadius.medium)
                    .strokeBorder(selected ? TomiColor.brand : TomiColor.border, lineWidth: selected ? 2 : TomiBorder.width)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? [.isSelected] : [])
    }
}

// MARK: Waiting

/// This side confirmed; the other one must confirm too. The way out stays visible.
private struct WaitingContent: View {
    let model: PairingViewModel

    var body: some View {
        let peerName = model.awaiting?.peer.name ?? ""
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s3) {
                Text("pair_waiting_title").tomiText(.title).foregroundStyle(TomiColor.text)
                    .accessibilityAddTraits(.isHeader)
                Text(L10n.text("pair_waiting_body", peerName)).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                TomiLinearProgress().padding(.top, TomiSpace.s2)
                ValidityText(deadline: model.validUntil)
            }
            .padding(.horizontal, TomiSpace.screen)
            BottomActions {
                Button(L10n.text("action_cancel")) { model.leave() }.buttonStyle(.tomiText)
            }
        }
    }
}
