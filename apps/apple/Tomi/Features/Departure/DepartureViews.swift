import SwiftUI
import TomiDesign
import TomiKit

// The departure of this device (Android `LeaveSpaceDialog`, `EraseWithoutProofDialog`, `DepartureBanner`; design
// 13-depart-espace): the banner at the top of the feed and the dialogs, driven by `LeaveSpaceModel`. Strong biometrics
// (Face ID / Touch ID only, every time) are asked here, then the model is told the outcome.

/// Asks for strong biometrics for an irreversible action of the departure.
@MainActor
enum DepartureAuth {
    static func deleteSpace(_ gate: BiometricGate) async -> AuthOutcome {
        await gate.authenticateStrong(reason: L10n.text("biometric_title_delete_space"))
    }

    static func erase(_ gate: BiometricGate) async -> AuthOutcome {
        await gate.authenticateStrong(reason: L10n.text("biometric_title_erase"))
    }
}

// MARK: Banner

/// What the departure is doing, at all times, at the top of the feed: never dismissable, read by VoiceOver when it
/// changes. Text buttons without icons, "Effacer quand même" in red.
struct DepartureBannerView: View {
    let model: LeaveSpaceModel
    let gate: BiometricGate

    var body: some View {
        if let content {
            VStack(alignment: .leading, spacing: TomiSpace.s2) {
                Text(content.title).tomiText(.subtitle).foregroundStyle(TomiColor.text)
                Text(content.message).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                    .fixedSize(horizontal: false, vertical: true)
                if content.progress { TomiLinearProgress() }
                if !content.buttons.isEmpty {
                    HStack(spacing: TomiSpace.s4) {
                        ForEach(content.buttons, id: \.label) { button in
                            Button(L10n.text(button.label), action: button.action)
                                .buttonStyle(TomiTextButtonStyle(color: button.destructive ? TomiColor.error : TomiColor.brand))
                                .fixedSize()
                                .opacity(button.enabled ? 1 : 0.38)
                                .disabled(!button.enabled)
                        }
                        Spacer(minLength: 0)
                    }
                }
            }
            .padding(TomiSpace.s4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(TomiColor.surfaceAlt, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
            .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width))
            .accessibilityElement(children: .contain)
            .onChange(of: content.message) { _, message in
                AccessibilityNotification.Announcement(message).post()
            }
        }
    }

    private struct BannerButton {
        let label: String
        var enabled = true
        var destructive = false
        let action: () -> Void
    }

    private struct Content {
        let title: String
        let message: String
        var progress = false
        var buttons: [BannerButton] = []
    }

    private var content: Content? {
        switch model.banner {
        case .none:
            return nil
        case let .pending(kind, last, running, authenticating):
            let message = switch last {
            case nil: "departure_pending_checking"
            case .network: "departure_pending_network"
            case .server: "departure_pending_server"
            case .notSent: "departure_pending_not_sent"
            case .notFound: "departure_pending_not_found"
            }
            var buttons = [
                BannerButton(label: "departure_retry", enabled: !running && !authenticating, action: retry),
                BannerButton(label: "departure_cancel", action: model.cancelFromBanner),
            ]
            if model.banner.offersEraseAnyway {
                buttons.append(BannerButton(label: "erase_anyway", destructive: true, action: model.openErase))
            }
            return Content(
                title: L10n.text(kind == .leave ? "departure_pending_leave_title" : "departure_pending_delete_title"),
                message: L10n.text(message), progress: running, buttons: buttons
            )
        case .wiping:
            return Content(title: L10n.text("departure_wiping"), message: "", progress: true)
        case .cancelling:
            return Content(title: L10n.text("departure_cancelling_title"), message: L10n.text("departure_cancelling"), progress: true)
        case .spaceNotFound:
            return Content(
                title: L10n.text("space_not_found_title"), message: L10n.text("space_not_found_message"),
                buttons: [BannerButton(label: "erase_anyway", destructive: true, action: model.openErase)]
            )
        case .revoked:
            return Content(
                title: L10n.text("space_revoked_title"), message: L10n.text("space_revoked_message"),
                buttons: [BannerButton(label: "leave_confirm", action: model.open)]
            )
        }
    }

    /// "Réessayer": a deletion asks for strong biometrics every time; a "Leave" is sent at once.
    private func retry() {
        guard model.beginRetry() else { return }
        Task {
            switch await DepartureAuth.deleteSpace(gate) {
            case .success: model.retryAuthenticated()
            case .unavailable: model.retryAuthEnded(unavailable: true)
            case .cancelled, .failed: model.retryAuthEnded(unavailable: false)
            }
        }
    }
}

// MARK: Dialogs

extension View {
    /// The dialogs of the departure, as a sheet that cannot be swiped away while a call runs.
    func departureDialogs(_ model: LeaveSpaceModel?, gate: BiometricGate) -> some View {
        modifier(DepartureDialogs(model: model, gate: gate))
    }
}

private struct DepartureDialogs: ViewModifier {
    let model: LeaveSpaceModel?
    let gate: BiometricGate

    func body(content: Content) -> some View {
        content.sheet(isPresented: Binding(
            get: { model.map { $0.dialog != .hidden } ?? false },
            set: { if !$0 { model?.dismiss() } }
        )) {
            if let model {
                DepartureSheet(model: model, gate: gate)
                    .presentationDetents([.medium, .large])
                    .presentationBackground(TomiColor.bg)
                    .interactiveDismissDisabled(model.locked)
            }
        }
    }
}

private extension LeaveSpaceModel {
    /// A call runs (or a prompt is up): the sheet is not dismissed by a gesture ("Annuler" is the way out).
    var locked: Bool {
        switch dialog {
        case .hidden: false
        case let .confirmLeave(state): state.busy
        case let .confirmDelete(state): state.busy || state.authenticating
        case let .eraseWithoutProof(state): (state.busy || state.authenticating) && !state.waiting
        }
    }
}

private struct DepartureSheet: View {
    let model: LeaveSpaceModel
    let gate: BiometricGate

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: TomiSpace.s3) {
                switch model.dialog {
                case .hidden:
                    EmptyView()
                case let .confirmLeave(state):
                    confirmLeave(state)
                case let .confirmDelete(state):
                    confirmDelete(state)
                case let .eraseWithoutProof(state):
                    eraseWithoutProof(state)
                }
            }
            .padding(TomiSpace.screen)
            .padding(.top, TomiSpace.s2)
        }
    }

    // "Se déconnecter de ton espace ?"
    @ViewBuilder
    private func confirmLeave(_ state: ConfirmLeave) -> some View {
        title("leave_title")
        Text("leave_message").tomiText(.body).foregroundStyle(TomiColor.textMuted)
        if state.busy {
            progress("leave_in_progress")
        }
        if let error = state.error {
            errorText(error == .updateRequired ? "leave_error_update" : "leave_error_unexpected")
        } else if state.failed {
            errorText("departure_failed")
        }
        actions {
            if state.busy {
                Button(L10n.text("departure_cancel"), action: model.cancelRunning).buttonStyle(.tomiText)
            } else {
                Button(L10n.text("leave_confirm"), action: model.confirmLeave).buttonStyle(.tomiDestructive)
                Button(L10n.text("action_cancel"), action: model.dismiss).buttonStyle(.tomiText)
            }
        }
    }

    // "Supprimer ton espace ?" (the only device)
    @ViewBuilder
    private func confirmDelete(_ state: ConfirmDelete) -> some View {
        title("delete_space_title")
        Text("delete_space_message").tomiText(.body).foregroundStyle(TomiColor.text)
        Text("delete_space_detail").tomiText(.body).foregroundStyle(TomiColor.textMuted)
        if state.busy { progress("delete_space_in_progress") }
        if state.biometricUnavailable {
            errorText("delete_space_unavailable_biometric")
        } else if let error = state.error {
            errorText(Self.deleteErrorKey(error))
        } else if state.failed {
            errorText("departure_failed")
        }
        actions {
            if state.busy {
                Button(L10n.text("departure_cancel"), action: model.cancelRunning).buttonStyle(.tomiText)
            } else {
                if state.error == .relayTooOld {
                    Button(L10n.text("erase_anyway"), action: model.openErase).buttonStyle(.tomiDestructive)
                } else {
                    Button(L10n.text("delete_space_confirm")) { deleteSpace() }
                        .buttonStyle(.tomiDestructive)
                        .disabled(!state.canConfirm)
                }
                Button(L10n.text("action_cancel"), action: model.dismiss).buttonStyle(.tomiText)
                    .disabled(state.authenticating)
            }
        }
    }

    // "Effacer ce téléphone sans confirmation du serveur ?"
    @ViewBuilder
    private func eraseWithoutProof(_ state: EraseWithoutProof) -> some View {
        title("erase_title")
        Text("erase_message").tomiText(.body).foregroundStyle(TomiColor.text)
        Text("erase_detail").tomiText(.body).foregroundStyle(TomiColor.textMuted)
        if state.waiting {
            progress("erase_waiting")
        } else if state.busy {
            progress("erase_in_progress")
        } else if state.checking {
            progress("erase_checking")
        }
        if state.biometricUnavailable {
            errorText("erase_unavailable_biometric")
        } else if state.expired {
            errorText("erase_expired")
        } else if state.notAllowed {
            errorText("erase_not_allowed")
        } else if state.failed {
            errorText("departure_failed")
        }
        // The whole row toggles the box.
        Button { model.setEraseChecked(!state.checked) } label: {
            HStack(spacing: TomiSpace.s3) {
                Image(systemName: state.checked ? "checkmark.square.fill" : "square")
                    .font(.system(size: 22))
                    .foregroundStyle(state.checked ? TomiColor.error : TomiColor.textMuted)
                Text("erase_checkbox").tomiText(.body).foregroundStyle(TomiColor.text)
                Spacer(minLength: 0)
            }
            .frame(minHeight: 48)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(state.busy || state.authenticating)
        .accessibilityAddTraits(state.checked ? .isSelected : [])
        actions {
            if state.notAllowed {
                Button(L10n.text("departure_retry"), action: model.recheckErase)
                    .buttonStyle(.tomiPrimary)
                    .disabled(state.checking)
            } else {
                Button(L10n.text("erase_confirm")) { erase() }
                    .buttonStyle(.tomiDestructive)
                    .disabled(!state.canConfirm || model.attemptRunning)
            }
            Button(L10n.text("action_cancel"), action: model.dismiss).buttonStyle(.tomiText)
                .disabled((state.busy || state.authenticating) && !state.waiting)
        }
    }

    private static func deleteErrorKey(_ error: RefusalReason) -> String {
        switch error {
        case .relayTooOld: "delete_space_error_relay_too_old"
        case .updateRequired: "delete_space_error_update"
        case .lastDevice, .unexpected: "delete_space_error_unexpected"
        }
    }

    private func deleteSpace() {
        guard model.beginDelete() else { return }
        Task {
            switch await DepartureAuth.deleteSpace(gate) {
            case .success: model.deleteAuthenticated()
            case .unavailable: model.authEnded(unavailable: true)
            case .cancelled, .failed: model.authEnded(unavailable: false)
            }
        }
    }

    private func erase() {
        Task {
            guard await model.beginErase() else { return }
            switch await DepartureAuth.erase(gate) {
            case .success: model.eraseAuthenticated()
            case .unavailable: model.authEnded(unavailable: true)
            case .cancelled, .failed: model.authEnded(unavailable: false)
            }
        }
    }

    private func title(_ key: LocalizedStringKey) -> some View {
        Text(key).tomiText(.title).foregroundStyle(TomiColor.text).accessibilityAddTraits(.isHeader)
    }

    private func progress(_ key: LocalizedStringKey) -> some View {
        VStack(alignment: .leading, spacing: TomiSpace.s2) {
            TomiLinearProgress()
            Text(key).tomiText(.body).foregroundStyle(TomiColor.textMuted)
        }
    }

    private func errorText(_ key: String) -> some View {
        Text(L10n.text(key)).tomiText(.body).foregroundStyle(TomiColor.error)
    }

    private func actions<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: TomiSpace.s1) { content() }.padding(.top, TomiSpace.s2)
    }
}
