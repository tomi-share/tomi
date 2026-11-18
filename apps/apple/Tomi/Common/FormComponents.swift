import SwiftUI
import TomiDesign

extension View {
    /// The box of a text field (Android outlined field): `surface`, 1 pt border, radius 8.
    func tomiFieldBox(focused: Bool = false) -> some View {
        padding(.horizontal, TomiSpace.s3)
            .padding(.vertical, TomiSpace.s3)
            .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
            .overlay(
                RoundedRectangle(cornerRadius: TomiRadius.medium)
                    .strokeBorder(focused ? TomiColor.brand : TomiColor.border, lineWidth: focused ? 2 : TomiBorder.width)
            )
    }
}

/// Secondary button of the charter: outlined, `brand` label, radius 8, 48 pt high (Android `OutlinedButton`).
struct TomiSecondaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        SecondaryBody(configuration: configuration)
    }

    private struct SecondaryBody: View {
        let configuration: Configuration
        @Environment(\.isEnabled) private var isEnabled

        var body: some View {
            configuration.label
                .tomiText(.label)
                .foregroundStyle(TomiColor.brand)
                .frame(maxWidth: .infinity, minHeight: 48)
                .background(configuration.isPressed ? TomiColor.brandContainer : .clear, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
                .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.borderStrong))
                .opacity(isEnabled ? 1 : 0.38)
        }
    }
}

/// Text button: `brand` label, 48 pt high, no frame (Android `TextButton`).
struct TomiTextButtonStyle: ButtonStyle {
    var color: Color = TomiColor.brand

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .tomiText(.label)
            .foregroundStyle(color)
            .frame(maxWidth: .infinity, minHeight: 48)
            .contentShape(Rectangle())
            .opacity(configuration.isPressed ? 0.6 : 1)
    }
}

extension ButtonStyle where Self == TomiSecondaryButtonStyle {
    static var tomiSecondary: TomiSecondaryButtonStyle { TomiSecondaryButtonStyle() }
}

extension ButtonStyle where Self == TomiTextButtonStyle {
    static var tomiText: TomiTextButtonStyle { TomiTextButtonStyle() }
}

/// A column whose content is centred in the height when it fits, and scrolls when it does not (Android
/// `CenteredScrollColumn`): a short screen keeps its block in the middle, a large text size still reaches everything.
struct CenteredScroll<Content: View>: View {
    var spacing: CGFloat = TomiSpace.s4
    var alignment: HorizontalAlignment = .leading
    @ViewBuilder let content: () -> Content

    var body: some View {
        GeometryReader { geo in
            ScrollView {
                VStack(alignment: alignment, spacing: spacing) { content() }
                    .frame(maxWidth: TomiLayout.readableWidth)
                    .frame(maxWidth: .infinity, minHeight: geo.size.height)
            }
            .scrollBounceBehavior(.basedOnSize)
        }
    }
}

extension String {
    /// Empty or only white space.
    var isBlank: Bool { trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
}
