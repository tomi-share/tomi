import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/render/ClipDetection.kt.

/// Decides from two measured heights whether a height-limited view clips its content. Used for Markdown and HTML,
/// which are bounded by a maximum height instead of a line count (plain text uses the text layout's own overflow flag).
public enum ClipDetection {
    /// One or two pixels of rounding (density scaling of a web view, line metrics) are not a truncation.
    public static let tolerancePx = 2

    /// `contentHeight`: full height of the content. `visibleHeight`: height the view actually got; 0 = not laid out yet.
    public static func isClipped(contentHeight: Int, visibleHeight: Int) -> Bool {
        visibleHeight > 0 && contentHeight > visibleHeight + tolerancePx
    }
}
