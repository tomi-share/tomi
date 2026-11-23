import SwiftUI
import TomiDesign

/// "Données illisibles" (Android `UnreadableDataScreen`): the Keychain key that protects the database is gone, or the
/// database cannot be read. "Réessayer" opens again (a Keychain not readable yet); "Réinitialiser Tomi" erases the
/// local data, behind a confirmation that says what it costs.
struct UnreadableDataScreen: View {
    let resetFailed: Bool
    let onRetry: () -> Void
    let onReset: () -> Void
    @State private var confirming = false

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            ScrollView {
                VStack(alignment: .leading, spacing: TomiSpace.s4) {
                    Image("TomiLogotype")
                        .renderingMode(.template)
                        .resizable()
                        .scaledToFit()
                        .frame(height: 32)
                        .foregroundStyle(TomiColor.logo)
                        .accessibilityLabel(Text("app_name"))
                    Text(L10n.text("unreadable_title"))
                        .tomiText(.title)
                        .foregroundStyle(TomiColor.text)
                        .padding(.top, TomiSpace.s2)
                        .accessibilityAddTraits(.isHeader)
                    Text(L10n.text("unreadable_message"))
                        .tomiText(.body)
                        .foregroundStyle(TomiColor.textMuted)
                    if resetFailed {
                        Text(L10n.text("ios_unreadable_reset_failed"))
                            .tomiText(.body)
                            .foregroundStyle(TomiColor.error)
                            .onAppear {
                                AccessibilityNotification.Announcement(L10n.text("ios_unreadable_reset_failed")).post()
                            }
                    }
                    Button(L10n.text("unreadable_retry"), action: onRetry).buttonStyle(.tomiPrimary)
                    Button(L10n.text("unreadable_reset")) { confirming = true }.buttonStyle(.tomiSecondary)
                }
                .padding(.horizontal, TomiSpace.screen)
                .padding(.vertical, TomiSpace.s4)
                .frame(maxWidth: 560, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
        }
        .alert(L10n.text("unreadable_reset_title"), isPresented: $confirming) {
            Button(L10n.text("action_cancel"), role: .cancel) {}
            Button(L10n.text("unreadable_reset_confirm"), role: .destructive, action: onReset)
        } message: {
            Text(L10n.text("unreadable_reset_message"))
        }
    }
}

#Preview {
    UnreadableDataScreen(resetFailed: true, onRetry: {}, onReset: {})
}

/// The Keychain refuses this build (`AppContainer.Phase.misconfigured`): said plainly, with no way to erase anything.
/// Seen by whoever builds the app, never by a user of a published build.
struct MisconfiguredScreen: View {
    /// The Keychain may only have been out of reach for a moment.
    let onRetry: () -> Void

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            VStack(alignment: .leading, spacing: TomiSpace.s3) {
                Text("ios_misconfigured_title").tomiText(.title).foregroundStyle(TomiColor.text)
                    .accessibilityAddTraits(.isHeader)
                Text("ios_misconfigured_body").tomiText(.body).foregroundStyle(TomiColor.textMuted)
                Button(L10n.text("unreadable_retry"), action: onRetry).buttonStyle(.tomiPrimary).padding(.top, TomiSpace.s2)
            }
            .padding(TomiSpace.s5)
            .frame(maxWidth: 520)
        }
    }
}
