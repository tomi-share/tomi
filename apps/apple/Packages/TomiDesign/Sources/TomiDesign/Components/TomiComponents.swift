import SwiftUI

/// Feed card container: surface, 1 pt border, radius 8, padding 12 / 16. A hidden item adds the
/// 3 pt plum accent on the left (`secretAccent: true`).
public struct TomiCard<Content: View>: View {
    private let secretAccent: Bool
    private let content: Content

    public init(secretAccent: Bool = false, @ViewBuilder content: () -> Content) {
        self.secretAccent = secretAccent
        self.content = content()
    }

    public var body: some View {
        content
            .padding(.vertical, TomiSpace.s3)
            .padding(.horizontal, TomiSpace.s4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(TomiColor.surface)
            .overlay(alignment: .leading) {
                if secretAccent {
                    Rectangle().fill(TomiColor.secret).frame(width: TomiBorder.secretAccent)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: TomiRadius.medium))
            .overlay(
                RoundedRectangle(cornerRadius: TomiRadius.medium)
                    .strokeBorder(TomiColor.border, lineWidth: TomiBorder.width)
            )
    }
}

/// Chip (filters, expiry, detected type): 32 pt high, full radius, `brandContainer` when selected.
public struct TomiChip: View {
    private let title: LocalizedStringKey
    private let icon: TomiIcon?
    private let selected: Bool
    private let action: () -> Void

    public init(_ title: LocalizedStringKey, icon: TomiIcon? = nil, selected: Bool = false, action: @escaping () -> Void) {
        self.title = title
        self.icon = icon
        self.selected = selected
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            HStack(spacing: TomiSpace.s1) {
                if let icon {
                    icon.image.font(.system(size: TomiIconSize.small))
                }
                Text(title).tomiText(.label)
            }
            .padding(.horizontal, TomiSpace.s3)
            .frame(minHeight: 32)
            .foregroundStyle(selected ? TomiColor.brand : TomiColor.text)
            .background(selected ? TomiColor.brandContainer : TomiColor.surface, in: Capsule())
            .overlay(Capsule().strokeBorder(selected ? Color.clear : TomiColor.borderStrong, lineWidth: TomiBorder.width))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// Main button: `brand` fill, `onBrand` label, radius 8, 48 pt high. `destructive`: `error` fill, for the
/// irreversible actions (leaving the space, erasing).
public struct TomiPrimaryButtonStyle: ButtonStyle {
    private let destructive: Bool

    public init(destructive: Bool = false) {
        self.destructive = destructive
    }

    public func makeBody(configuration: Configuration) -> some View {
        PrimaryButtonBody(configuration: configuration, fill: destructive ? TomiColor.error : TomiColor.brand)
    }

    private struct PrimaryButtonBody: View {
        let configuration: Configuration
        let fill: Color
        @Environment(\.isEnabled) private var isEnabled

        var body: some View {
            configuration.label
                .tomiText(.label)
                .foregroundStyle(TomiColor.onBrand)
                .frame(maxWidth: .infinity, minHeight: 48)
                .background(fill, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
                .opacity(isEnabled ? (configuration.isPressed ? 0.85 : 1) : 0.38)
        }
    }
}

extension ButtonStyle where Self == TomiPrimaryButtonStyle {
    public static var tomiPrimary: TomiPrimaryButtonStyle { TomiPrimaryButtonStyle() }
    public static var tomiDestructive: TomiPrimaryButtonStyle { TomiPrimaryButtonStyle(destructive: true) }
}

/// Linear progress (charter §2.5 and §6.3: never a circular indicator). With `value` it shows a
/// fraction; without, a short segment travels the track (still when Reduce Motion is on).
public struct TomiLinearProgress: View {
    private let value: Double?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(value: Double? = nil) {
        self.value = value.map { min(max($0, 0), 1) }
    }

    public var body: some View {
        GeometryReader { geo in
            let width = geo.size.width
            ZStack(alignment: .leading) {
                Capsule().fill(TomiColor.brandContainer)
                if let value {
                    Capsule().fill(TomiColor.brand).frame(width: width * value)
                } else if reduceMotion {
                    Capsule().fill(TomiColor.brand).frame(width: width * 0.3)
                } else {
                    TimelineView(.animation) { context in
                        let t = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1.4) / 1.4
                        Capsule()
                            .fill(TomiColor.brand)
                            .frame(width: width * 0.3)
                            .offset(x: (width * 1.3) * t - width * 0.3)
                    }
                }
            }
            .clipShape(Capsule())
        }
        .frame(height: 4)
        .accessibilityElement()
        .accessibilityValue(value.map { Text("\(Int(($0 * 100).rounded())) %") } ?? Text(""))
    }
}

/// Feedback surface of snackbars and the send banner: inverse surface, radius 8, shadow.
public struct TomiFeedbackBanner<Content: View>: View {
    private let content: Content

    public init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    public var body: some View {
        content
            .tomiText(.body)
            .foregroundStyle(TomiColor.onInverseSurface)
            .padding(.horizontal, TomiSpace.s4)
            .frame(maxWidth: .infinity, minHeight: 48, alignment: .leading)
            .background(TomiColor.inverseSurface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
            .shadow(color: .black.opacity(0.18), radius: 8, y: 3)
            .padding(.horizontal, TomiSpace.s4)
    }
}
