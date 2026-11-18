import SwiftUI
import TomiDesign

/// First launch after pairing (Android `OnboardingScreen` v2, design 02-onboarding-v2): how to keep Tomi at the front of
/// the share sheet, and a button that opens the share sheet with a harmless sample so the user can do it right away.
/// iOS wording: Tomi is added to the favourites of the share sheet (Partager → Plus → Modifier), there is no long press
/// as on Android. Android's second step ("Tomi est-il en tête ?") comes with the share extension (PR-12): until then
/// Tomi is not in the share sheet. `fromHelp`: opened again from the menu of the feed, without "Plus tard".
struct OnboardingScreen: View {
    var fromHelp = false
    let onDone: () -> Void

    var body: some View {
        ZStack {
            TomiColor.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                CenteredScroll(spacing: TomiSpace.s4) {
                    TomiIcon.pin.image
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
                        SystemShare.present(text: L10n.text("onboarding_sample_text"), subject: nil, sensitive: false)
                    }
                    .buttonStyle(.tomiPrimary)
                    if !fromHelp {
                        Button(L10n.text("onboarding_later"), action: onDone).buttonStyle(.tomiText)
                    }
                }
            }
        }
        .navigationTitle(Text("ios_onboarding_title"))
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(true)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button(action: onDone) { TomiIcon.close.image }
                    .accessibilityLabel(Text("action_close"))
            }
        }
    }
}
