import Foundation

// Port of the pure part of apps/android/app/src/main/java/eu/shaffe/tomi/ui/render/ProgressiveColumn.kt: the weight of a
// text and the number of parts of the first screen. Android reveals the parts of a long text by hand and reserves
// their room itself (Compose cannot nest a lazy list in a scrolling column); on iOS the column is a lazy stack
// (`ChunkedMarkdown`, in the app), so only what decides which parts are prepared before the screen shows is kept.

/// Estimates for a column of text parts built while scrolling.
public enum ProgressiveReveal {
    /// Parts prepared at once are those that fill the first screen, by their estimated height: usually one (a part is
    /// about two screens tall); a short first part (a title before a long table) takes the next ones with it. Never
    /// more than this many.
    public static let maxInitialItems = 4

    /// Height of one unit of weight (pt), before anything is measured: see `textWeight`.
    public static let estimatedPointsPerWeight = 0.5

    /// What a line break weighs, in characters: a line is at least one line high, however short.
    public static let lineWeight = 20

    /// A document up to this length is cut into chunks at once; a longer one off the main thread.
    public static let syncChunkingChars = 8_000

    /// A text shown as is up to this length is cut into pieces of lines at once; a longer one off the main thread.
    public static let syncSourceChars = 64_000

    /// Weight of `text`, for the height estimates: its characters (about 40 a line of body text, 22 pt high) plus
    /// `lineWeight` per line break. UTF-16 code units, as Android counts.
    public static func textWeight(_ text: String) -> Int {
        var units = 0
        var lines = 0
        for unit in text.utf16 {
            units += 1
            if unit == 0x0A { lines += 1 }
        }
        return units + lineWeight * lines
    }

    /// The weight a screen `height` points tall holds.
    public static func screenWeight(height: Double) -> Double {
        height / estimatedPointsPerWeight
    }

    /// How many of the `count` parts make the first screen: the first one, then the next ones while the estimated
    /// height of those before them does not fill a screen (`screenWeight` = the weight a screen holds),
    /// `maxInitialItems` at most.
    public static func initialItems(count: Int, weight: (Int) -> Int, screenWeight: Double) -> Int {
        var shown = 0
        var total = 0
        while shown < min(count, maxInitialItems) && (shown == 0 || Double(total) < screenWeight) {
            total += max(weight(shown), 1)
            shown += 1
        }
        return shown
    }
}
