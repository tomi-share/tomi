import CoreText
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// JetBrains Mono for exact text: a URL, a revealed secret, a code, a file name, a command must read EXACTLY as it is.
/// The font draws `://` (and `->`, `==`…) through its contextual alternates (`calt`), which showed `https://` as
/// `https: //`: here `calt` and `liga` are off (Android `TomiFonts.NO_LIGATURES`). A revealed secret also gets the
/// slashed zero (`zero`), so that 0 and O never mix up. The code viewer keeps its own fonts.
public enum TomiMonoFont {
    /// The OpenType settings, for `kCTFontFeatureSettingsAttribute`.
    static func featureSettings(slashedZero: Bool) -> [[String: Any]] {
        var settings = [feature("calt", 0), feature("liga", 0)]
        if slashedZero { settings.append(feature("zero", 1)) }
        return settings
    }

    private static func feature(_ tag: String, _ value: Int) -> [String: Any] {
        [kCTFontOpenTypeFeatureTag as String: tag, kCTFontOpenTypeFeatureValue as String: value]
    }

    /// The font at `size` points (already scaled for Dynamic Type), the system monospaced font when JetBrains Mono is
    /// missing (never a crash).
    public static func ctFont(size: CGFloat, slashedZero: Bool = false) -> CTFont {
        TomiFonts.register()
        let settings = featureSettings(slashedZero: slashedZero)
        let descriptor = CTFontDescriptorCreateWithAttributes([
            kCTFontFamilyNameAttribute as String: TomiTokens.FontFamily.mono,
            kCTFontFeatureSettingsAttribute as String: settings,
        ] as CFDictionary)
        let font = CTFontCreateWithFontDescriptor(descriptor, size, nil)
        // A missing family gives Helvetica, proportional: fall back on the system fixed-pitch font instead.
        if CTFontCopyFamilyName(font) as String == TomiTokens.FontFamily.mono { return font }
        let system = CTFontCreateUIFontForLanguage(.userFixedPitch, size, nil)
            ?? CTFontCreateWithName("Menlo" as CFString, size, nil)
        let features = CTFontDescriptorCreateWithAttributes([kCTFontFeatureSettingsAttribute as String: settings] as CFDictionary)
        return CTFontCreateCopyWithAttributes(system, size, nil, features)
    }

    /// The SwiftUI font at `size` points, scaled for `dynamicTypeSize` like the body text (as `Font.custom` does).
    public static func font(size: CGFloat, slashedZero: Bool = false, dynamicTypeSize: DynamicTypeSize = .large) -> Font {
        Font(ctFont(size: scaled(size, dynamicTypeSize), slashedZero: slashedZero))
    }

    static func scaled(_ size: CGFloat, _ dynamicTypeSize: DynamicTypeSize) -> CGFloat {
        #if canImport(UIKit)
        let traits = UITraitCollection(preferredContentSizeCategory: UIContentSizeCategory(dynamicTypeSize))
        return UIFontMetrics(forTextStyle: .body).scaledValue(for: size, compatibleWith: traits)
        #else
        return size
        #endif
    }
}

extension View {
    /// Exact mono text (`TomiMonoFont`) at `size` points, following Dynamic Type.
    public func tomiMono(size: CGFloat, slashedZero: Bool = false) -> some View {
        modifier(TomiMonoModifier(size: size, slashedZero: slashedZero))
    }
}

private struct TomiMonoModifier: ViewModifier {
    let size: CGFloat
    let slashedZero: Bool
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    func body(content: Content) -> some View {
        content.font(TomiMonoFont.font(size: size, slashedZero: slashedZero, dynamicTypeSize: dynamicTypeSize))
    }
}
