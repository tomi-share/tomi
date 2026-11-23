import SwiftUI
import TomiDesign

/// First launch after pairing (Android `OnboardingScreen` v2, design 02-onboarding-v2): how to keep Tomi at the front of
/// the share sheet, and a button that opens the share sheet with a harmless sample so the user can do it right away.
/// iOS wording: Tomi is added to the favourites of the share sheet (Partager → Plus → Modifier), there is no long press
/// as on Android. Android's second step ("Tomi est-il en tête ?") comes with the share extension (PR-12): until then
/// Tomi is not in the share sheet. `fromHelp`: opened again from the menu of the feed, without "Plus tard".
///
/// On the Mac the same screen says what a menu bar app needs said (the icon beside the clock, the global shortcut, the
/// opening with the session: the Mac wording of the same texts), and its button opens the settings.
struct OnboardingScreen: View {
    var fromHelp = false
    let onDone: () -> Void
    #if os(macOS)
    @Environment(\.macSurface) private var macSurface
    #endif

    /// The pin of the share sheet on the phones; the menu bar on the Mac.
    private static var symbol: Image {
        #if os(macOS)
        Image(systemName: "menubar.arrow.up.rectangle")
        #else
        TomiIcon.pin.image
        #endif
    }

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                CenteredScroll(spacing: TomiSpace.s4) {
                    Self.symbol
                        .font(.system(size: 44, weight: .light))
                        .foregroundStyle(TomiColor.brand)
                        .accessibilityHidden(true)
                    Text("ios_onboarding_headline").tomiText(.title).foregroundStyle(TomiColor.text)
                        .accessibilityAddTraits(.isHeader)
                    Text("ios_onboarding_step_1").tomiText(.body).foregroundStyle(TomiColor.text)
                    Text("ios_onboarding_step_2").tomiText(.body).foregroundStyle(TomiColor.text)
                    Text("ios_onboarding_step_3").tomiText(.body).foregroundStyle(TomiColor.text)
                }
                .padding(.horizontal, TomiSpace.s5)
                BottomActions {
                    Button(L10n.text("onboarding_open_share")) {
                        #if os(macOS)
                        macSurface?.openSettings()
                        #else
                        SystemShare.present(text: L10n.text("onboarding_sample_text"), subject: nil, sensitive: false)
                        #endif
                    }
                    .buttonStyle(.tomiPrimary)
                    if !fromHelp {
                        Button(L10n.text("onboarding_later"), action: onDone).buttonStyle(.tomiText)
                    }
                }
            }
        }
        .navigationTitle(Text("ios_onboarding_title"))
        .inlineNavigationTitle()
        .navigationBarBackButtonHidden(true)
        .toolbar {
            ToolbarItem(placement: .tomiLeading) {
                Button(action: onDone) { TomiIcon.close.image }
                    .barControl()
                    .accessibilityLabel(Text("action_close"))
            }
        }
    }
}
