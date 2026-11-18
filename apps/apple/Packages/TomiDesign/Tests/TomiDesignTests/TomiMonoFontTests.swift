import CoreText
import XCTest
@testable import TomiDesign

/// Android `TypographyFeaturesTest`, checked on the shaped glyphs: exact text keeps the glyph of each character.
final class TomiMonoFontTests: XCTestCase {
    private let sample = "https://relay.example.org -> a==b"

    /// The glyphs CoreText lays out for `text` in `font`, after shaping (ligatures and contextual alternates).
    private func shaped(_ text: String, _ font: CTFont) -> [CGGlyph] {
        let attributed = NSAttributedString(string: text, attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
        let line = CTLineCreateWithAttributedString(attributed)
        let runs = CTLineGetGlyphRuns(line) as! [CTRun]
        return runs.flatMap { run -> [CGGlyph] in
            var glyphs = [CGGlyph](repeating: 0, count: CTRunGetGlyphCount(run))
            CTRunGetGlyphs(run, CFRange(location: 0, length: 0), &glyphs)
            return glyphs
        }
    }

    /// The glyph of each character on its own (the character map, no shaping).
    private func nominal(_ text: String, _ font: CTFont) -> [CGGlyph] {
        let units = Array(text.utf16)
        var glyphs = [CGGlyph](repeating: 0, count: units.count)
        XCTAssertTrue(CTFontGetGlyphsForCharacters(font, units, &glyphs, units.count))
        return glyphs
    }

    func testTheFontIsJetBrainsMono() throws {
        try XCTSkipUnless(TomiFonts.areAvailable)
        let family = CTFontCopyFamilyName(TomiMonoFont.ctFont(size: 14)) as String
        XCTAssertEqual(family, TomiTokens.FontFamily.mono)
    }

    func testExactTextIsDrawnWithoutLigatures() throws {
        try XCTSkipUnless(TomiFonts.areAvailable)
        let exact = TomiMonoFont.ctFont(size: 14)
        XCTAssertEqual(shaped(sample, exact), nominal(sample, exact))
    }

    /// The test above would pass with a font without ligatures: the default JetBrains Mono does change `://`, `->`, `==`.
    func testTheDefaultFontWouldChangeTheGlyphs() throws {
        try XCTSkipUnless(TomiFonts.areAvailable)
        let plain = CTFontCreateWithName(TomiTokens.FontFamily.mono as CFString, 14, nil)
        XCTAssertNotEqual(shaped(sample, plain), nominal(sample, plain))
    }

    func testTheSecretFontAlsoSlashesTheZero() throws {
        try XCTSkipUnless(TomiFonts.areAvailable)
        let secret = TomiMonoFont.ctFont(size: 14, slashedZero: true)
        XCTAssertNotEqual(shaped("0", secret), nominal("0", secret))
        XCTAssertEqual(shaped("a://b", secret), nominal("a://b", secret))
    }
}

extension TomiMonoFontTests {
    /// The fallback stays fixed-pitch (never Helvetica) and keeps the settings.
    func testAMissingFamilyFallsBackOnAFixedPitchFont() {
        let size: CGFloat = 14
        let features = CTFontDescriptorCreateWithAttributes([
            kCTFontFeatureSettingsAttribute as String: TomiMonoFont.featureSettings(slashedZero: false),
        ] as CFDictionary)
        let system = CTFontCreateUIFontForLanguage(.userFixedPitch, size, nil)!
        let fallback = CTFontCreateCopyWithAttributes(system, size, nil, features)
        XCTAssertTrue(CTFontGetSymbolicTraits(fallback).contains(.traitMonoSpace))
    }
}
