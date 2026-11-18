package eu.shaffe.tomi.core

/**
 * [fresh] (a new read of the whole feed) with what did not change taken from [previous] (the read before): an item that is equal
 * is the SAME instance, and an item whose metadata only changed (read, deliveries, pin, archive) keeps the instance of its
 * content (texts, attachments, thumbnails).
 *
 * The core hands out new objects for the whole feed at every change it announces (a sync announces many). The screens compare
 * by reference what they cannot compare cheaply: with the instances kept, a card whose item did not change is not composed
 * again, its thumbnails are not decoded again and its texts are not hashed or compared again. Equal by value in every case:
 * only the identity differs from [fresh].
 */
internal fun reuseUnchanged(previous: List<TomiItem>, fresh: List<TomiItem>): List<TomiItem> {
    if (previous.isEmpty() || fresh.isEmpty()) return fresh
    val known = HashMap<String, TomiItem>(previous.size * 2)
    for (item in previous) known[item.id] = item
    return fresh.map { item ->
        val old = known[item.id]
        when {
            old == null || old.content != item.content -> item
            else -> {
                // Same content: the comparison of the rest is then cheap (the content is compared by reference first).
                val shared = item.copy(content = old.content)
                if (shared == old) old else shared
            }
        }
    }
}
