import SwiftUI
import TomiDesign
import TomiKit

/// "Crée ton espace" (Android `CreateSpaceScreen`, design 01-pairing-v1-iter2 and 09-appairage-etats): the install
/// token by QR code, paste or typing, the name of this device, then "Ton espace est prêt". The scan fills the field and
/// submits nothing; back closes the scanner first.
struct CreateSpaceScreen: View {
    @Bindable var model: CreateSpaceViewModel
    let onJoinInstead: () -> Void
    let onFinished: () -> Void
    @Environment(\.dismiss) private var dismiss

    @State private var scanning = false
    @State private var cameraDenied = false
    @State private var revealed = false
    @State private var showsHelp = false
    @State private var editingName = false
    @State private var nameDraft = ""
    @FocusState private var tokenFocused: Bool

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            if model.done {
                PairedContent(
                    title: "create_done_title",
                    message: L10n.text("create_done_body", model.deviceName),
                    onFinished: onFinished
                )
            } else {
                form
            }
        }
        .navigationBarBackButtonHidden(true)
        .toolbar {
            // Not while the space is being created: the call goes on and would leave the user outside.
            if !model.done && !model.busy {
                EntryBackButton {
                    if scanning { scanning = false } else { dismiss() }
                }
            }
        }
        .sheet(isPresented: $showsHelp) { WhereIsTheKeySheet() }
        .alert(L10n.text("create_name_label"), isPresented: $editingName) {
            TextField(L10n.text("create_name_label"), text: $nameDraft)
            Button(L10n.text("action_save")) { model.onDeviceNameChange(nameDraft) }
            Button(L10n.text("action_cancel"), role: .cancel) {}
        }
        .task { await model.loadDeviceRelay() }
        .onDisappear { model.clear() }
        .sensitiveScreen()
        .animation(.easeOut(duration: TomiMotion.medium), value: model.done)
    }

    private var form: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: TomiSpace.s4) {
                    Text("create_title").tomiText(.title).foregroundStyle(TomiColor.text)
                        .accessibilityAddTraits(.isHeader)
                    Text("create_intro").tomiText(.body).foregroundStyle(TomiColor.textMuted)
                    scan
                    divider
                    tokenField
                    Button { showsHelp = true } label: {
                        Label(L10n.text("create_where_button"), systemImage: "info.circle")
                    }
                    .buttonStyle(.tomiText)
                    nameCard
                    // The relay that gets the token: required when the device has none, folded otherwise.
                    RelayField(
                        text: model.relayText, onChange: { model.onRelayChange($0) },
                        required: model.relayFieldRequired, deviceAuthority: model.deviceRelayAuthority,
                        disabled: model.busy
                    )
                    // The relay retained, in full, before the token leaves the device (spec/pairing-v1.md 10.3).
                    if let authority = model.relayChoice.authority {
                        VStack(alignment: .leading, spacing: TomiSpace.s1) {
                            RelayCheckCard(authority: authority, label: "create_relay_check_label", hint: "create_relay_check_hint")
                            if model.relayFromLink {
                                Text("create_relay_from_link").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                            }
                        }
                    }
                    if let error = model.error {
                        Text(L10n.text(error)).tomiText(.body).foregroundStyle(TomiColor.error)
                    }
                    if model.spaceExists {
                        Button(L10n.text("create_join_instead"), action: onJoinInstead).buttonStyle(.tomiText)
                    }
                    // Charter 6.3: progress is linear, never a spinner.
                    if model.busy { TomiLinearProgress() }
                }
                .padding(TomiSpace.screen)
                .frame(maxWidth: TomiLayout.readableWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .scrollDismissesKeyboard(.interactively)
            BottomActions {
                Button(L10n.text("create_submit")) { Task { await model.submit() } }
                    .buttonStyle(.tomiPrimary)
                    .disabled(model.busy || model.token.isBlank)
            }
        }
    }

    @ViewBuilder
    private var scan: some View {
        if scanning {
            VStack(spacing: TomiSpace.s2) {
                ScannerPreview(height: 280, description: "create_scan_preview_description") { value in
                    scanning = false
                    model.onQRScanned(value)
                }
                Button(L10n.text("create_scan_cancel")) { scanning = false }.buttonStyle(.tomiText)
            }
        } else {
            Button {
                Task {
                    let granted = await QRScanner.requestAccess()
                    cameraDenied = !granted
                    scanning = granted
                }
            } label: {
                Label(L10n.text("create_scan_button"), systemImage: TomiIcon.scanQR.systemName)
            }
            .buttonStyle(.tomiSecondary)
            .disabled(model.busy || !QRScanner.isAvailable)
            .accessibilityLabel(Text("create_scan_button_description"))
        }
        CameraProblem(denied: cameraDenied, deniedMessage: "create_camera_denied", unavailableMessage: "ios_create_camera_unavailable")
    }

    private var divider: some View {
        HStack(spacing: TomiSpace.s3) {
            Rectangle().fill(TomiColor.border).frame(height: 1)
            Text("create_or_paste").tomiText(.caption).foregroundStyle(TomiColor.textMuted).fixedSize()
            Rectangle().fill(TomiColor.border).frame(height: 1)
        }
        .accessibilityElement(children: .combine)
    }

    /// Masked by default; shown in clear in mono without ligatures. No correction, no suggestion, no password saving.
    private var tokenField: some View {
        let binding = Binding(get: { model.token }, set: { model.onTokenChange($0) })
        return VStack(alignment: .leading, spacing: TomiSpace.s1) {
            Text("create_token_label").tomiText(.label).foregroundStyle(TomiColor.text)
            HStack(spacing: TomiSpace.s2) {
                Group {
                    if revealed {
                        TextField("", text: binding).tomiText(.mono)
                    } else {
                        SecureField("", text: binding)
                    }
                }
                .noAutocapitalization()
                .autocorrectionDisabled()
                .textContentType(nil)
                .submitLabel(.next)
                .focused($tokenFocused)
                .disabled(model.busy)
                .accessibilityLabel(Text("create_token_label"))
                Button { revealed.toggle() } label: {
                    (revealed ? TomiIcon.masked : TomiIcon.reveal).image.foregroundStyle(TomiColor.textMuted)
                        .frame(width: 32, height: 32)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(revealed ? "action_hide" : "action_reveal"))
            }
            .tomiFieldBox(focused: tokenFocused)
        }
    }

    private var nameCard: some View {
        TomiCard {
            HStack(spacing: TomiSpace.s3) {
                SystemActions.thisDeviceIcon.font(.system(size: 20)).foregroundStyle(TomiColor.textMuted)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text("create_name_label").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                    Text(verbatim: model.deviceName).tomiText(.subtitle).foregroundStyle(TomiColor.text)
                }
                .accessibilityElement(children: .combine)
                Spacer(minLength: 0)
                Button {
                    nameDraft = model.deviceName
                    editingName = true
                } label: {
                    Image(systemName: "pencil").foregroundStyle(TomiColor.text).frame(width: 44, height: 44)
                }
                .buttonStyle(.plain)
                .disabled(model.busy)
                .accessibilityLabel(Text("create_name_edit"))
            }
        }
    }
}

/// "Où trouver la clé ?": where the relay showed the token, and the command that gives another one.
private struct WhereIsTheKeySheet: View {
    @Environment(\.dismiss) private var dismiss
    /// Not translated (Android `create_where_command`, translatable="false").
    private static let command = "$ tomi-relay admin invite --new"

    var body: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s3) {
            Text("create_where_title").tomiText(.title).foregroundStyle(TomiColor.text)
                .accessibilityAddTraits(.isHeader)
            Text("create_where_body").tomiText(.body).foregroundStyle(TomiColor.textMuted)
            Text("create_where_other_title").tomiText(.subtitle).foregroundStyle(TomiColor.text)
                .padding(.top, TomiSpace.s2)
            Text("create_where_run").tomiText(.body).foregroundStyle(TomiColor.textMuted)
            Text(verbatim: Self.command)
                .tomiMono(size: 14)
                .foregroundStyle(TomiColor.text)
                .padding(TomiSpace.s3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(TomiColor.surfaceAlt, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
                .textSelection(.disabled)
            Spacer(minLength: 0)
            Button(L10n.text("create_where_ok")) { dismiss() }.buttonStyle(.tomiPrimary)
        }
        .padding(TomiSpace.screen)
        .presentationDetents([.medium, .large])
        .presentationBackground(TomiColor.bg)
    }
}

#if DEBUG
#Preview("Crée ton espace") {
    NavigationStack {
        CreateSpaceScreen(
            model: CreateSpaceViewModel(core: PreviewCore(items: [], localDevice: PreviewFixtures.phone), suggestedName: "Alice's iPhone"),
            onJoinInstead: {}, onFinished: {}
        )
    }
}
#endif
