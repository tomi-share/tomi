package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.TomiItem

/**
 * What the search mode of the feed shows around the results. Pure rules, no content of any item is read: for the note
 * about the hidden ("Masqué") items only the boolean `secret` is looked at (golden rule 3).
 */
object FeedSearchDisplay {

    /** A blank query is no search (see [FeedFiltering.apply]). */
    fun isSearching(query: String): Boolean = query.isNotBlank()

    /** The result counter is shown for a real query or an active filter; with an empty query and the "All" filter it is noise. */
    fun showsCounter(query: String, filter: FeedFilter): Boolean = isSearching(query) || filter != FeedFilter.All

    /** The note "hidden items are searched without their title nor content" only matters when one of them is among the results. */
    fun showsMaskedNote(query: String, results: List<TomiItem>): Boolean = isSearching(query) && results.any { it.secret }

    /**
     * Results of a search shown flat, without section titles: pinned first, then the others. Each keeps the behaviour of
     * its section (the pinned ones are not swipeable, the others are), which is why the two lists stay apart.
     */
    fun isFlat(query: String): Boolean = isSearching(query)
}
