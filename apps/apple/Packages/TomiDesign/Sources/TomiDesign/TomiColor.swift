import SwiftUI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Colours of the charter (design/tomi/brand-guidelines.md §4), each following the light / dark
/// appearance of the system. Rules: `error` only for real errors; `sun` only for the symbol on a
/// `night` background (never text, never a button); no `secondary`, `secret`, `success` or
/// `error` text on a `brand` background.
public enum TomiColor {
    public static let bg = dynamic(TomiTokens.ColorLight.bg, TomiTokens.ColorDark.bg)
    public static let surface = dynamic(TomiTokens.ColorLight.surface, TomiTokens.ColorDark.surface)
    public static let surfaceAlt = dynamic(TomiTokens.ColorLight.surfaceAlt, TomiTokens.ColorDark.surfaceAlt)
    public static let text = dynamic(TomiTokens.ColorLight.text, TomiTokens.ColorDark.text)
    public static let textMuted = dynamic(TomiTokens.ColorLight.textMuted, TomiTokens.ColorDark.textMuted)
    public static let border = dynamic(TomiTokens.ColorLight.border, TomiTokens.ColorDark.border)
    public static let borderStrong = dynamic(TomiTokens.ColorLight.borderStrong, TomiTokens.ColorDark.borderStrong)
    public static let brand = dynamic(TomiTokens.ColorLight.brand, TomiTokens.ColorDark.brand)
    public static let onBrand = dynamic(TomiTokens.ColorLight.onBrand, TomiTokens.ColorDark.onBrand)
    public static let brandContainer = dynamic(TomiTokens.ColorLight.brandContainer, TomiTokens.ColorDark.brandContainer)
    /// Ochre: waiting states (offline, queued, "Annuler" of the send banner). Never an error.
    public static let secondary = dynamic(TomiTokens.ColorLight.secondary, TomiTokens.ColorDark.secondary)
    /// Plum: marker of hidden ("Masqué") items.
    public static let secret = dynamic(TomiTokens.ColorLight.secret, TomiTokens.ColorDark.secret)
    public static let secretContainer = dynamic(TomiTokens.ColorLight.secretContainer, TomiTokens.ColorDark.secretContainer)
    public static let success = dynamic(TomiTokens.ColorLight.success, TomiTokens.ColorDark.success)
    public static let error = dynamic(TomiTokens.ColorLight.error, TomiTokens.ColorDark.error)

    /// Inverse surface of snackbars and banners (text colour of the other appearance).
    public static let inverseSurface = dynamic(TomiTokens.ColorLight.text, TomiTokens.ColorDark.text)
    public static let onInverseSurface = dynamic(TomiTokens.ColorLight.bg, TomiTokens.ColorDark.bg)
    /// Ochre and brand readable on the inverse surface (the values of the other appearance).
    public static let inverseSecondary = dynamic(TomiTokens.ColorDark.secondary, TomiTokens.ColorLight.secondary)
    public static let inverseBrand = dynamic(TomiTokens.ColorDark.brand, TomiTokens.ColorLight.brand)
    /// A selected button of a floating bar: `brand` at 24 %, visible on the bar in light and dark (Android `ToolbarIconButton`).
    public static let barSelected = brand.opacity(0.24)

    /// Colour of the logo: night on a light background, white on a dark one (charter §4.3).
    public static let logo = dynamic(TomiTokens.ColorIdentity.night, 0xFFFFFF)

    /// Identity colours, for the app icon and launch screen only.
    public static let night = Color(rgb: TomiTokens.ColorIdentity.night)
    public static let sun = Color(rgb: TomiTokens.ColorIdentity.sun)

    static func dynamic(_ light: UInt32, _ dark: UInt32) -> Color {
        #if canImport(UIKit)
        Color(UIColor { trait in
            UIColor(rgb: trait.userInterfaceStyle == .dark ? dark : light)
        })
        #else
        Color(NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            return NSColor(rgb: isDark ? dark : light)
        })
        #endif
    }
}

extension Color {
    /// Colour from a `0xRRGGBB` token.
    public init(rgb: UInt32) {
        self.init(
            .sRGB,
            red: Double((rgb >> 16) & 0xFF) / 255,
            green: Double((rgb >> 8) & 0xFF) / 255,
            blue: Double(rgb & 0xFF) / 255
        )
    }
}

#if canImport(UIKit)
extension UIColor {
    convenience init(rgb: UInt32) {
        self.init(
            red: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: 1
        )
    }
}
#else
extension NSColor {
    convenience init(rgb: UInt32) {
        self.init(
            srgbRed: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: 1
        )
    }
}
#endif

/// WCAG 2.x contrast ratio between two `0xRRGGBB` colours (1 to 21).
public func tomiContrastRatio(_ a: UInt32, _ b: UInt32) -> Double {
    func luminance(_ rgb: UInt32) -> Double {
        func channel(_ v: UInt32) -> Double {
            let c = Double(v) / 255
            return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * channel((rgb >> 16) & 0xFF) + 0.7152 * channel((rgb >> 8) & 0xFF)
            + 0.0722 * channel(rgb & 0xFF)
    }
    let (la, lb) = (luminance(a), luminance(b))
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
}
