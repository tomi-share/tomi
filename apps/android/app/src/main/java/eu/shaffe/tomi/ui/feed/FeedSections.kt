package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.TomiItem
import java.time.Instant

/** The feed as the user sees it: a "Pinned" section on top, then the other items newest first. */
data class FeedSections(val pinned: List<TomiItem>, val recent: List<TomiItem>) {
    val isEmpty: Boolean get() = pinned.isEmpty() && recent.isEmpty()

    companion object {
        /**
         * Builds the sections from the (already filtered) items of the core, which arrive newest first
         * (spec/pin-contract.md: the core never reorders for a pin, the UI does). Pinned items are sorted by
         * `pinnedAt`, most recently pinned first; ties and a missing `pinnedAt` fall back to the creation time.
         * An item is in exactly one section.
         */
        fun split(items: List<TomiItem>): FeedSections {
            val (pinned, recent) = items.partition { it.pinned }
            return FeedSections(
                pinned = pinned.sortedWith(
                    compareByDescending<TomiItem> { it.pinnedAt ?: Instant.MIN }.thenByDescending { it.createdAt },
                ),
                recent = recent,
            )
        }
    }
}
