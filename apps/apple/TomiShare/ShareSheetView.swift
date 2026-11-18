import SwiftUI
import TomiDesign
import TomiKit

/// What the share extension shows (Android `ShareContent`). iOS presents an extension as a full sheet over the source
/// app: the Tomi header, what leaves (never the content of a secret), and at the bottom the "Envoyé" banner or the
/// buttons of the settings.
struct ShareSheetView: View {
    let flow: ShareFlow
    let onClose: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                content
                    .padding(.horizontal, TomiSpace.screen)
                    .padding(.vertical, TomiSpace.s3)
                    .frame(maxWidth: TomiLayout.readableWidth)
                    .frame(maxWidth: .infinity)
            }
            bottom
                .padding(TomiSpace.screen)
                .frame(maxWidth: TomiLayout.readableWidth)
        }
        .background(TomiColor.bg.ignoresSafeArea())
        .animation(.easeOut(duration: TomiMotion.medium), value: flow.state)
    }

    private var header: some View {
        HStack(spacing: TomiSpace.s2) {
            TomiSymbol().fill(TomiColor.logo).frame(width: 32, height: 32).accessibilityHidden(true)
            Text("app_name").tomiText(.title).foregroundStyle(TomiColor.text)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, TomiSpace.screen)
        .padding(.top, TomiSpace.s4)
        .frame(maxWidth: TomiLayout.readableWidth)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }

    @ViewBuilder
    private var content: some View {
        switch flow.state {
        case .loading:
            ProgressView().frame(maxWidth: .infinity).padding(.top, TomiSpace.s5)
        case .nothingToShare:
            Color.clear.frame(height: 1).onAppear(perform: onClose)
        case .notPaired:
            message("share_not_paired")
        case .unreadable:
            message("share_unreadable")
        case .tooLate:
            message("share_too_late")
        case .failed:
            message("share_failed")
        case let .sent(sent):
            SharePreview(parsed: sent.parsed, secret: sent.secret)
        case let .confirm(draft, locked):
            ShareSettings(flow: flow, draft: draft, secretLocked: locked, info: "share_secret_detected")
        case let .options(draft):
            ShareSettings(flow: flow, draft: draft, secretLocked: false, info: nil)
        }
    }

    @ViewBuilder
    private var bottom: some View {
        switch flow.state {
        case .loading, .nothingToShare:
            EmptyView()
        case .notPaired, .unreadable, .tooLate, .failed:
            Button(L10n.text("action_close"), action: onClose).buttonStyle(.tomiPrimary)
        case let .sent(sent):
            SentBanner(
                deadline: sent.deadline, skippedFiles: sent.skippedFiles, undoing: flow.busy,
                onUndo: { Task { if await flow.undo() { onClose() } } },
                onOptions: { Task { await flow.openOptions() } }
            )
            // The window ends: the share stays in the inbox, the app sends it.
            .task(id: sent.id) {
                try? await Task.sleep(for: .milliseconds(sent.deadline.remainingMs(MonotonicClock.nowMs())))
                guard !Task.isCancelled, case let .sent(current) = flow.state, current.id == sent.id else { return }
                onClose()
            }
        case .confirm, .options:
            VStack(spacing: TomiSpace.s1) {
                Button(L10n.text("action_send")) { Task { await flow.confirm() } }.buttonStyle(.tomiPrimary)
                // In "Options" the first send is already cancelled: closing sends nothing.
                Button(L10n.text(isOptions ? "action_cancel_send" : "action_cancel"), action: onClose).buttonStyle(.tomiText)
            }
        }
    }

    private var isOptions: Bool {
        if case .options = flow.state { return true }
        return false
    }

    private func message(_ key: LocalizedStringKey) -> some View {
        Text(key)
            .tomiText(.body)
            .foregroundStyle(TomiColor.text)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// What leaves: title, link, the start of the text, the number of files; dots for a secret.
private struct SharePreview: View {
    let parsed: ParsedShare
    let secret: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s1) {
            if secret {
                Text("label_masked").tomiText(.label).foregroundStyle(TomiColor.secret)
                Text(verbatim: L10n.text("preview_secret_masked")).tomiText(.body).foregroundStyle(TomiColor.secret)
            } else {
                if let title = parsed.title {
                    Text(verbatim: title).tomiText(.subtitle).foregroundStyle(TomiColor.text).lineLimit(2)
                }
                if let url = parsed.url {
                    Text(verbatim: url).tomiMono(size: 13).foregroundStyle(TomiColor.brand).lineLimit(2)
                }
                if let text = parsed.text {
                    Text(verbatim: String(text.plain.prefix(300))).tomiText(.body).foregroundStyle(TomiColor.text).lineLimit(6)
                }
                if !parsed.files.isEmpty {
                    Label(L10n.text("preview_files", parsed.files.count), systemImage: "doc")
                        .tomiText(.caption)
                        .foregroundStyle(TomiColor.textMuted)
                }
            }
        }
        .padding(TomiSpace.s4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(secret ? TomiColor.secretContainer : TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.large))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.large).strokeBorder(TomiColor.border))
    }
}

/// The settings before it leaves (Android `SendOptionsSheet`): the "Masquer le contenu" switch and the expiry.
private struct ShareSettings: View {
    let flow: ShareFlow
    let draft: ShareDraft
    let secretLocked: Bool
    let info: LocalizedStringKey?

    var body: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s3) {
            if let info {
                Text(info).tomiText(.body).foregroundStyle(TomiColor.text)
            }
            SharePreview(parsed: draft.parsed, secret: draft.secret)
            Toggle(isOn: Binding(get: { draft.secret }, set: { flow.onSecretChange($0) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("label_secret").tomiText(.label).foregroundStyle(TomiColor.text)
                    Text(draft.secret ? "label_secret_on" : "label_secret_off").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                }
            }
            .tint(TomiColor.brand)
            .disabled(secretLocked)
            Text(L10n.text("expiry_label")).tomiText(.label).foregroundStyle(TomiColor.text)
            ExpiryChips(options: draft.expiryOptions, selected: draft.expiry) { flow.onExpiryChange($0) }
            if draft.secret && !draft.hasAttachment {
                Text("expiry_after_read_hint").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
            }
        }
    }
}
