package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.TomiItem

/** [Archives] is the only filter that shows archived items; they are absent from every other one. */
enum class FeedFilter { All, Links, Text, Files, Secrets, Archives }

/**
 * Whether the UI treats the item as archived. Pin and archive exclude each other, so a core never reports both; if two
 * devices ever converge on both (spec/archive-contract.md), the pin wins and the item stays visible.
 */
fun TomiItem.isShownAsArchived(): Boolean = archived && !pinned

object FeedFiltering {

    /**
     * Archived items belong to [FeedFilter.Archives] only; every other filter (All included) leaves them out, so they do
     * not appear in "Recent" nor in the typed filters.
     */
    fun matches(item: TomiItem, filter: FeedFilter): Boolean {
        if (filter == FeedFilter.Archives) return item.isShownAsArchived()
        if (item.isShownAsArchived()) return false
        return when (filter) {
            FeedFilter.All -> true
            // A hidden item stays under "Masqués" only: classing it by its content would reveal what it holds (link, text, files).
            FeedFilter.Links -> !item.secret && item.url != null
            FeedFilter.Text -> !item.secret && item.text != null
            FeedFilter.Files -> !item.secret && item.attachments.isNotEmpty()
            FeedFilter.Secrets -> item.secret
            FeedFilter.Archives -> false
        }
    }

    /**
     * Local, case-insensitive search. A hidden ("Masqué") item is searched by its source device only: neither its
     * title nor its content (text, link, file names) is ever searched, so typing in the search field cannot be used to
     * guess or to confirm anything about a hidden item, whether it is currently shown in clear or not.
     */
    fun matchesQuery(item: TomiItem, query: String): Boolean {
        val q = query.trim()
        if (q.isEmpty()) return true
        val fields = buildList {
            add(item.sourceDevice.name)
            if (!item.secret) {
                add(item.title)
                add(item.url)
                add(item.text?.plain)
                item.attachments.forEach { add(it.name) }
            }
        }
        return fields.any { it?.contains(q, ignoreCase = true) == true }
    }

    /**
     * Filter then search. An archived item is "findable": while the user types a search under [FeedFilter.All], archived
     * items are searched too (the card carries the archive badge). With an empty search they only show under
     * [FeedFilter.Archives].
     */
    fun apply(items: List<TomiItem>, filter: FeedFilter, query: String): List<TomiItem> {
        val searchingEverywhere = filter == FeedFilter.All && query.isNotBlank()
        return items.filter { item ->
            (matches(item, filter) || (searchingEverywhere && item.isShownAsArchived())) && matchesQuery(item, query)
        }
    }
}
