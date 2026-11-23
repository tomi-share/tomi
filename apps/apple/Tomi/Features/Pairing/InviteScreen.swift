import CoreImage.CIFilterBuiltins
import SwiftUI
import TomiDesign
import TomiKit

/// "Ajouter un appareil" (Android `InviteScreen`, design 10-ajouter-appareil): the invitation code and its QR code, the
/// verification code of the new device, the wait for the other person, then "Appareil ajouté". Back from the comparison
/// is a refusal (a new code is made); from anywhere else it ends the invitation.
struct InviteScreen: View {
    let core: any TomiCore
    /// Built once, when the screen shows (not at every evaluation of the destination).
    @State private var model: InviteViewModel?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            if let model { content(model) }
        }
        .navigationTitle(Text("invite_title"))
        .inlineNavigationTitle()
        .navigationBarBackButtonHidden(true)
        .toolbar { EntryBackButton(action: back) }
        .onAppear {
            if model == nil { model = InviteViewModel(core: core) }
            model?.start()
        }
        .onDisappear { model?.abort() }
        .sensitiveScreen()
        .animation(.easeOut(duration: TomiMotion.medium), value: model?.state)
    }

    @ViewBuilder
    private func content(_ model: InviteViewModel) -> some View {
        switch model.state {
        case .starting:
            Waiting(message: "invite_starting", detail: nil, onCancel: leave)
        case let .showCode(code, link, deadline, notice):
            ShowCode(code: code, link: link, deadline: deadline, notice: notice, onCancel: leave)
        case let .verify(peer, verificationCode):
            Verify(peer: peer, code: verificationCode, onAdd: model.add, onRefuse: model.refuse)
        case let .adding(peer):
            Waiting(message: "invite_adding_title", detail: L10n.text("invite_adding_body", peer.name), onCancel: leave)
        case let .done(peer, historyAnnounced):
            Done(peer: peer, historyAnnounced: historyAnnounced) { dismiss() }
        case let .failed(reason):
            Failed(reason: reason, onRetry: model.restart) { dismiss() }
        }
    }

    private func back() {
        if case .verify = model?.state { model?.refuse() } else { leave() }
    }

    private func leave() {
        model?.abort()
        dismiss()
    }
}

// MARK: States

/// The channel opening, or the other person confirming: a linear progress and the way out.
private struct Waiting: View {
    let message: LocalizedStringKey
    let detail: String?
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s3) {
                Text(message).tomiText(detail == nil ? .body : .title).foregroundStyle(TomiColor.text)
                if let detail { Text(verbatim: detail).tomiText(.body).foregroundStyle(TomiColor.textMuted) }
                // Charter 6.3: progress is linear, never a spinner.
                TomiLinearProgress().padding(.top, TomiSpace.s2)
            }
            .padding(.horizontal, TomiSpace.screen)
            BottomActions { Button(L10n.text("action_cancel"), action: onCancel).buttonStyle(.tomiText) }
        }
    }
}

/// The code and the QR code: the same secret, two ways to enter it. Shown, never copied (no selection, no copy).
private struct ShowCode: View {
    let code: String
    let link: String
    let deadline: ContinuousClock.Instant
    let notice: String?
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s3, alignment: .center) {
                if let notice {
                    Text(L10n.text(notice)).tomiText(.body).foregroundStyle(TomiColor.error).multilineTextAlignment(.center)
                }
                Text("invite_intro").tomiText(.body).foregroundStyle(TomiColor.textMuted).multilineTextAlignment(.center)
                // The invitation lets a device into the space: the QR and the code are left out of captures.
                QRCodeImage(content: link)
                    .frame(maxWidth: 280)
                    .accessibilityLabel(Text("invite_qr_description"))
                    .captureProtected()
                Text(verbatim: code)
                    .tomiMono(size: 24)
                    .fontWeight(.semibold)
                    .foregroundStyle(TomiColor.text)
                    .multilineTextAlignment(.center)
                    .textSelection(.disabled)
                    .accessibilityLabel(Text(verbatim: code.replacingOccurrences(of: "-", with: " ")))
                    .captureProtected()
                ValidityText(deadline: deadline)
                Text("invite_single_use").tomiText(.caption).foregroundStyle(TomiColor.textMuted).multilineTextAlignment(.center)
            }
            .padding(.horizontal, TomiSpace.screen)
            BottomActions { Button(L10n.text("action_cancel"), action: onCancel).buttonStyle(.tomiText) }
        }
    }
}

/// The verification code of the new device: compare it with the one on its screen, then add it.
private struct Verify: View {
    let peer: PairingPeer
    let code: String
    let onAdd: () -> Void
    let onRefuse: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s2) {
                Text("pair_verify_title").tomiText(.title).foregroundStyle(TomiColor.text)
                    .accessibilityAddTraits(.isHeader)
                Text(L10n.text("invite_verify_body", peer.name)).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                HStack(spacing: TomiSpace.s3) {
                    (peer.platform == .android || peer.platform == .ios ? TomiIcon.phone : TomiIcon.computer).image
                        .foregroundStyle(TomiColor.text)
                        .frame(width: 40, height: 40)
                        .background(TomiColor.surfaceAlt, in: Circle())
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(verbatim: peer.name).tomiText(.subtitle).foregroundStyle(TomiColor.text)
                        Text(L10n.text(peer.platform.labelKey)).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                    }
                }
                .accessibilityElement(children: .combine)
                .padding(.top, TomiSpace.s4)
                VerificationDigits(code: code).padding(.top, TomiSpace.s5)
                // If the other device shows an error instead of a code (the code was already used), the answer is "no".
                Text("invite_verify_hint")
                    .tomiText(.caption)
                    .foregroundStyle(TomiColor.textMuted)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.top, TomiSpace.s2)
            }
            .padding(.horizontal, TomiSpace.screen)
            BottomActions {
                Button(L10n.text("invite_add"), action: onAdd).buttonStyle(.tomiPrimary)
                Button(L10n.text("pair_verify_different"), action: onRefuse).buttonStyle(.tomiText)
            }
        }
    }
}

private struct Done: View {
    let peer: PairingPeer
    let historyAnnounced: Bool
    let onFinished: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s3, alignment: .center) {
                TomiIcon.delivered.image
                    .font(.system(size: 64, weight: .light))
                    .foregroundStyle(TomiColor.success)
                    .accessibilityHidden(true)
                Text("pair_done_title").tomiText(.title).foregroundStyle(TomiColor.text)
                    .accessibilityAddTraits(.isHeader)
                Text(L10n.text("pair_done_body", peer.name)).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                    .multilineTextAlignment(.center)
                if historyAnnounced {
                    Text(L10n.text("invite_done_history", peer.name)).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                        .multilineTextAlignment(.center)
                }
            }
            .padding(.horizontal, TomiSpace.s5)
            BottomActions { Button(L10n.text("action_done"), action: onFinished).buttonStyle(.tomiPrimary) }
        }
    }
}

private struct Failed: View {
    let reason: PairingFailure
    let onRetry: () -> Void
    let onClose: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            CenteredScroll(spacing: TomiSpace.s3) {
                Text("invite_failed_title").tomiText(.title).foregroundStyle(TomiColor.text)
                    .accessibilityAddTraits(.isHeader)
                Text(L10n.text(PairingErrors.inviteMessageKey(reason))).tomiText(.body).foregroundStyle(TomiColor.textMuted)
            }
            .padding(.horizontal, TomiSpace.screen)
            BottomActions {
                Button(L10n.text("invite_new_code"), action: onRetry).buttonStyle(.tomiPrimary)
                Button(L10n.text("action_close"), action: onClose).buttonStyle(.tomiText)
            }
        }
    }
}

/// A QR code of `content`, black on white whatever the theme (scanners read it best), with its quiet zone. Rendered once
/// per content, at a whole number of pixels per module so that every module has the same size.
private struct QRCodeImage: View {
    let content: String
    @State private var rendered: (content: String, image: PlatformImage?)?

    var body: some View {
        let image = rendered?.content == content ? rendered?.image : nil
        Group {
            if let image {
                Image(platformImage: image).interpolation(.none).resizable().scaledToFit()
            } else {
                TomiIcon.scanQR.image.font(.system(size: 96, weight: .ultraLight)).foregroundStyle(TomiColor.textMuted)
            }
        }
        .padding(TomiSpace.s4)
        .background(Color.white, in: RoundedRectangle(cornerRadius: TomiRadius.large))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.large).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width))
        .onAppear { renderIfNeeded() }
        .onChange(of: content) { renderIfNeeded() }
    }

    private func renderIfNeeded() {
        guard rendered?.content != content else { return }
        rendered = (content, Self.render(content))
    }

    private static func render(_ content: String) -> PlatformImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(content.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        // 10 pixels per module: sharp at any size it is drawn.
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        guard let cgImage = CIContext().createCGImage(scaled, from: scaled.extent) else { return nil }
        return PlatformImage(cgImage: cgImage)
    }
}
