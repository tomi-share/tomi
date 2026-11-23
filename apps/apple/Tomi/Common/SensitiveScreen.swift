import SwiftUI
import TomiDesign

extension View {
    /// Marks a screen that shows a secret (Android `FLAG_SECURE`): the app is covered in the app switcher and while the
    /// screen is recorded or mirrored. iOS cannot block a screenshot: after one, revealed items are masked again.
    func sensitiveScreen(_ active: Bool = true) -> some View {
        modifier(SensitiveScreenMarker(active: active))
    }
}

private struct SensitiveScreenMarker: ViewModifier {
    let active: Bool
    @Environment(AppContainer.self) private var container
    @State private var counted = false
    /// On screen (not under a pushed screen).
    @State private var visible = false

    func body(content: Content) -> some View {
        content
            #if os(macOS)
            // macOS has no cover to draw: the whole window leaves the captures while the screen is shown (A6).
            .captureProtected(counted)
            #endif
            .onAppear { visible = true; update(active) }
            .onChange(of: active) { _, now in update(visible && now) }
            .onDisappear { visible = false; update(false) }
    }

    private func update(_ now: Bool) {
        guard now != counted else { return }
        counted = now
        if now { container.sensitiveScreenAppeared() } else { container.sensitiveScreenDisappeared() }
    }
}

/// What covers the app while a secret is on screen and the app leaves it, or the screen is captured. In the app
/// switcher, the logotype alone. While the screen is shared or recorded (the app on screen), it says why, how to stop
/// the sharing (the cover goes away by itself then) and gives a way out: the secrets are masked and the app goes back
/// to the feed, or to the welcome screen during a pairing. It never traps the user.
struct PrivacyCover: View {
    var captured = false
    /// Joining or creating a space: what is hidden is the pairing code.
    var pairing = false
    var onLeave: () -> Void = {}
    /// Developer builds only: goes on despite the sharing (nil in Release builds).
    var onAllowForDev: (() -> Void)?

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            if captured {
                capturedContent
            } else {
                logotype
            }
        }
    }

    private var logotype: some View {
        Image("TomiLogotype")
            .renderingMode(.template)
            .resizable()
            .scaledToFit()
            .frame(height: 32)
            .foregroundStyle(TomiColor.logo)
            .accessibilityHidden(true)
    }

    private var capturedContent: some View {
        GeometryReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: TomiSpace.s5) {
                    Image(systemName: "rectangle.inset.filled.and.person.filled")
                        .font(.system(size: 36, weight: .regular))
                        .foregroundStyle(TomiColor.brand)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: TomiSpace.s3) {
                        Text(L10n.text("ios_capture_title"))
                            .tomiText(.title)
                            .foregroundStyle(TomiColor.text)
                            .accessibilityAddTraits(.isHeader)
                        Text(L10n.text(pairing ? "ios_capture_cover_pairing" : "ios_capture_cover"))
                            .tomiText(.body)
                            .foregroundStyle(TomiColor.textMuted)
                    }
                    steps
                    VStack(spacing: TomiSpace.s3) {
                        Button(L10n.text(pairing ? "ios_capture_cover_back" : "ios_capture_cover_close"), action: onLeave)
                            .buttonStyle(.tomiPrimary)
                        if let onAllowForDev {
                            // Never in a published build (see `AppContainer.allowCaptureForDev`).
                            Button(action: onAllowForDev) {
                                Text(verbatim: "Continuer malgré le partage (build de développement)")
                                    .multilineTextAlignment(.center)
                            }
                            .buttonStyle(.tomiText)
                        }
                    }
                }
                .padding(.horizontal, TomiSpace.screen)
                .padding(.vertical, TomiSpace.s6)
                .frame(maxWidth: 480, alignment: .leading)
                .frame(maxWidth: .infinity, minHeight: proxy.size.height)
            }
        }
    }

    /// How to stop the sharing, in a frame of its own.
    private var steps: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s3) {
            Text(L10n.text("ios_capture_steps_title"))
                .tomiText(.label)
                .foregroundStyle(TomiColor.text)
            step("hand.tap", "ios_capture_step_island")
            step("macbook.and.iphone", "ios_capture_step_mac")
            step("record.circle", "ios_capture_step_record")
            Text(L10n.text("ios_capture_resume"))
                .tomiText(.caption)
                .foregroundStyle(TomiColor.textMuted)
                .padding(.top, TomiSpace.s1)
        }
        .padding(TomiSpace.s4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border))
    }

    private func step(_ symbol: String, _ key: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: TomiSpace.s3) {
            Image(systemName: symbol)
                .font(.system(size: 15))
                .foregroundStyle(TomiColor.textMuted)
                .frame(width: 22)
                .accessibilityHidden(true)
            Text(L10n.text(key))
                .tomiText(.body)
                .foregroundStyle(TomiColor.text)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

#Preview("Partage, appairage") {
    PrivacyCover(captured: true, pairing: true, onAllowForDev: {})
}

#Preview("Partage, contenu masqué") {
    PrivacyCover(captured: true)
}
