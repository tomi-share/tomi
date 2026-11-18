import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/FeedSearchDisplay.kt.

/// What the search mode of the feed shows around the results. Pure rules, no content of any item is read: for the note
/// about the hidden ("Masqué") items only the flag `secret` is looked at (golden rule 3).
public enum FeedSearchDisplay {
    /// A blank query is no search (see `FeedFiltering.apply`).
    public static func isSearching(_ query: String) -> Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// The result counter is shown for a real query or an active filter; with an empty query and "Tous" it is noise.
    public static func showsCounter(query: String, filter: FeedFilter) -> Bool {
        isSearching(query) || filter != .all
    }

    /// The note "les Masqués sont cherchés sans leur titre ni leur contenu" only matters when one of them is among the
    /// results of a real query.
    public static func showsMaskedNote(query: String, results: [TomiItem]) -> Bool {
        isSearching(query) && results.contains { $0.secret }
    }

    /// Results of a search are shown flat, without section titles: pinned first, then the others. Each keeps the
    /// behaviour of its section (pinned cards do not swipe, the others do), which is why the two lists stay apart.
    public static func isFlat(query: String) -> Bool { isSearching(query) }
}
