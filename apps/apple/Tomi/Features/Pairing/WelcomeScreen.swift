import SwiftUI
import TomiDesign
import TomiKit

/// First screen of a device that is not in a space yet (Android `WelcomeScreen`, design 01-pairing-v1-iter2): says
/// what Tomi does and opens one of two doors, create the space or join it. The doors sit at the bottom, under the thumb;
/// on a screen too small for that, the page scrolls. Nothing secret is shown or typed here.
struct WelcomeScreen: View {
    let onCreateSpace: () -> Void
    let onJoinSpace: () -> Void

    var body: some View {
        GeometryReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: TomiSpace.s4) {
                    Image("TomiLogotype")
                        .renderingMode(.template)
                        .resizable()
                        .scaledToFit()
                        .frame(height: 32)
                        .foregroundStyle(TomiColor.logo)
                        .accessibilityLabel(Text("app_name"))
                    Text("welcome_title").tomiText(.title).foregroundStyle(TomiColor.text)
                        .accessibilityAddTraits(.isHeader)
                    Text("welcome_intro").tomiText(.body).foregroundStyle(TomiColor.textMuted)
                    // Pushes the doors down to the thumb when the screen is tall enough.
                    Spacer(minLength: TomiSpace.s5)
                    Text("welcome_question").tomiText(.subtitle).foregroundStyle(TomiColor.text)
                        .accessibilityAddTraits(.isHeader)
                    DoorCard(icon: "plus", title: "welcome_create_title", hint: "welcome_create_hint", action: onCreateSpace)
                    DoorCard(icon: "arrow.right.to.line", title: "welcome_join_title", hint: "welcome_join_hint", action: onJoinSpace)
                }
                .padding(TomiSpace.screen)
                .frame(maxWidth: TomiLayout.readableWidth, minHeight: proxy.size.height, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
        }
        .background(TomiColor.bg.ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
    }
}

/// One door: round icon, title, what it takes and a chevron; read as one button.
private struct DoorCard: View {
    let icon: String
    let title: String
    let hint: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: TomiSpace.s3) {
                Image(systemName: icon)
                    .font(.system(size: 18, weight: .medium))
                    .foregroundStyle(TomiColor.brand)
                    .frame(width: 40, height: 40)
                    .background(TomiColor.brandContainer, in: Circle())
                VStack(alignment: .leading, spacing: TomiSpace.s1) {
                    Text(L10n.text(title)).tomiText(.subtitle).foregroundStyle(TomiColor.text)
                    Text(L10n.text(hint)).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                TomiIcon.readMore.image.font(.system(size: 14, weight: .semibold)).foregroundStyle(TomiColor.textMuted)
            }
            .padding(TomiSpace.s4)
            .frame(minHeight: 72)
            .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
            .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(L10n.text("welcome_door_description", L10n.text(title), L10n.text(hint))))
        .accessibilityAddTraits(.isButton)
    }
}

#if DEBUG
#Preview("Bienvenue") {
    NavigationStack { WelcomeScreen(onCreateSpace: {}, onJoinSpace: {}) }
}
#endif
