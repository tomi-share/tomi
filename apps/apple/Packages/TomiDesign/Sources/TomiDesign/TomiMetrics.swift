import SwiftUI

/// Spacing scale of the charter (4, 8, 12, 16, 24, 32).
public enum TomiSpace {
    public static let s1 = CGFloat(TomiTokens.Space.step1)
    public static let s2 = CGFloat(TomiTokens.Space.step2)
    public static let s3 = CGFloat(TomiTokens.Space.step3)
    public static let s4 = CGFloat(TomiTokens.Space.step4)
    public static let s5 = CGFloat(TomiTokens.Space.step5)
    public static let s6 = CGFloat(TomiTokens.Space.step6)
    /// Margin of a screen and inner horizontal padding of a card.
    public static let screen = s4
}

/// Corner radii of the charter (cards and buttons 8, sheets 12). The Android app keeps a debug
/// switch to the rounder Material 3 Expressive radii; the Apple apps use the charter only.
public enum TomiRadius {
    /// Code block, check box.
    public static let small = CGFloat(TomiTokens.Radius.sm)
    /// Buttons, fields, feed cards, hidden block.
    public static let medium = CGFloat(TomiTokens.Radius.md)
    /// Sheets, popovers, drop zone, the send button.
    public static let large = CGFloat(TomiTokens.Radius.lg)
}

public enum TomiBorder {
    public static let width = CGFloat(TomiTokens.Border.width)
    /// Left accent of a hidden ("Masqué") card, in `TomiColor.secret`.
    public static let secretAccent = CGFloat(TomiTokens.BorderWidth.secretAccent)
}

public enum TomiIconSize {
    public static let small = CGFloat(TomiTokens.IconSize.sm)
    public static let medium = CGFloat(TomiTokens.IconSize.md)
    public static let large = CGFloat(TomiTokens.IconSize.lg)
}

/// Durations of the charter, and the product timings kept with the tokens.
public enum TomiMotion {
    public static let short = Double(TomiTokens.MotionDuration.short) / 1000
    public static let medium = Double(TomiTokens.MotionDuration.medium) / 1000
    /// One lap of the pearl (logo animation), never looped.
    public static let logoLap = Double(TomiTokens.MotionLogo.lap) / 1000
    /// Undo window of a send ("Envoyé · Annuler").
    public static let undoWindow = Double(TomiTokens.Behavior.undoWindowMs) / 1000
    /// A copied hidden item leaves the clipboard after this delay.
    public static let secretClipboardClear = Double(TomiTokens.Behavior.secretClipboardClearMs) / 1000
}
