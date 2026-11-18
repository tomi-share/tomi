import CoreText
import SwiftUI

/// Fonts of the charter (§5): Inter for the interface, JetBrains Mono for code, revealed passwords,
/// URLs, file names and pairing codes. Both are bundled (SIL OFL 1.1, licences next to the files),
/// never loaded from the network. Call [`TomiFonts.register()`] once at start-up.
public enum TomiFonts {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var registered = false

    /// Registers the bundled fonts for this process. Idempotent; a font that cannot be
    /// registered leaves the system font in its place (never a crash).
    public static func register() {
        lock.lock()
        defer { lock.unlock() }
        guard !registered else { return }
        registered = true
        for name in ["Inter-Variable", "JetBrainsMono-Variable"] {
            guard let url = Bundle.module.url(forResource: name, withExtension: "ttf", subdirectory: "Fonts")
            else { continue }
            CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
    }

    /// `true` once both families are available (tests, diagnostics).
    public static var areAvailable: Bool {
        register()
        #if canImport(UIKit)
        return !UIFont.fontNames(forFamilyName: TomiTokens.FontFamily.ui).isEmpty
            && !UIFont.fontNames(forFamilyName: TomiTokens.FontFamily.mono).isEmpty
        #else
        let families = NSFontManager.shared.availableFontFamilies
        return families.contains(TomiTokens.FontFamily.ui) && families.contains(TomiTokens.FontFamily.mono)
        #endif
    }
}

/// Text styles of the charter (§5): size / line height / weight. No weight above 600.
public enum TomiTextStyle: CaseIterable, Sendable {
    case title, subtitle, body, label, caption, mono, pairingCode

    public var size: CGFloat {
        switch self {
        case .title: CGFloat(TomiTokens.FontSize.title)
        case .subtitle: CGFloat(TomiTokens.FontSize.subtitle)
        case .body: CGFloat(TomiTokens.FontSize.body)
        case .label: CGFloat(TomiTokens.FontSize.label)
        case .caption: CGFloat(TomiTokens.FontSize.caption)
        case .mono: CGFloat(TomiTokens.FontSize.mono)
        case .pairingCode: CGFloat(TomiTokens.FontSize.pairingCode)
        }
    }

    public var lineHeight: CGFloat {
        switch self {
        case .title: CGFloat(TomiTokens.FontLine.title)
        case .subtitle: CGFloat(TomiTokens.FontLine.subtitle)
        case .body: CGFloat(TomiTokens.FontLine.body)
        case .label: CGFloat(TomiTokens.FontLine.label)
        case .caption: CGFloat(TomiTokens.FontLine.caption)
        case .mono: CGFloat(TomiTokens.FontLine.mono)
        case .pairingCode: CGFloat(TomiTokens.FontSize.pairingCode) * 1.2
        }
    }

    public var weight: Font.Weight {
        switch self {
        case .title, .subtitle, .pairingCode: .semibold
        case .label: .medium
        case .body, .caption, .mono: .regular
        }
    }

    public var isMono: Bool { self == .mono || self == .pairingCode }

    /// Scales with Dynamic Type, relative to the closest system style.
    var relativeTo: Font.TextStyle {
        switch self {
        case .title: .title3
        case .subtitle: .headline
        case .body, .mono: .body
        case .label: .subheadline
        case .caption: .caption
        case .pairingCode: .largeTitle
        }
    }

    public var font: Font {
        let family = isMono ? TomiTokens.FontFamily.mono : TomiTokens.FontFamily.ui
        var font = Font.custom(family, size: size, relativeTo: relativeTo).weight(weight)
        if self == .pairingCode { font = font.monospacedDigit() }
        return font
    }
}

extension View {
    /// Applies a charter text style (font and line height).
    /// `.mono` is exact text: no ligature (`TomiMonoFont`).
    public func tomiText(_ style: TomiTextStyle) -> some View {
        modifier(TomiTextModifier(style: style))
            .lineSpacing(max(0, style.lineHeight - style.size * 1.2))
    }
}

private struct TomiTextModifier: ViewModifier {
    let style: TomiTextStyle
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    func body(content: Content) -> some View {
        if style == .mono {
            content.font(TomiMonoFont.font(size: style.size, dynamicTypeSize: dynamicTypeSize))
        } else {
            content.font(style.font)
        }
    }
}
