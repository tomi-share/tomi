package eu.shaffe.tomi.core

import eu.shaffe.tomi.ui.feed.ArchiveAction
import eu.shaffe.tomi.ui.feed.ArchiveSwipe
import eu.shaffe.tomi.ui.feed.FeedFilter
import eu.shaffe.tomi.ui.feed.FeedFiltering
import eu.shaffe.tomi.ui.feed.FeedSections
import eu.shaffe.tomi.ui.feed.isShownAsArchived
import eu.shaffe.tomi.ui.render.HtmlSanitizer
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Clock
import java.time.Duration
import java.time.Instant
import java.time.ZoneOffset

/**
 * Archive / unarchive on the fake core (contract of spec/archive-contract.md), the filtering that hides archived
 * items outside the "Archives" filter, the exclusion with the pin, and the dispatch of the swipe to the core.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class ArchiveTest {
    private val office = DeviceRef("pc", "PC-Bureau")
    private val local = DeviceRef(FakeTomiCore.LOCAL_ID, "Pixel 8")
    private val base = Instant.parse("2026-11-04T10:00:00Z")

    /** Clock that moves by one minute at each reading, so two decisions never share a timestamp. */
    private class SteppingClock(private var now: Instant) : Clock() {
        override fun getZone() = ZoneOffset.UTC
        override fun withZone(zone: java.time.ZoneId?) = this
        override fun instant(): Instant = now.also { now = now.plus(Duration.ofMinutes(1)) }
    }

    private fun core() = FakeTomiCore(CoroutineScope(Job()), clock = SteppingClock(base), startPaired = true)

    private fun item(id: String, archived: Boolean = false, pinned: Boolean = false, build: (String) -> TomiItem = { plain(it) }) =
        build(id).copy(
            archived = archived, archivedAt = if (archived) base else null,
            pinned = pinned, pinnedAt = if (pinned) base else null,
        )

    private fun plain(id: String) = readableItem(id, office, base, text = RichText("text $id"))

    // ---- FakeTomiCore: archive / unarchive -------------------------------------------------------------------------

    @Test
    fun archiveSetsArchivedAndArchivedAtAndUnarchiveClearsThem() = runTest {
        val core = core()
        val before = core.getItem("demo-text-1")!!
        assertFalse(before.archived)
        assertNull(before.archivedAt)

        core.archive("demo-text-1")
        val archived = core.getItem("demo-text-1")!!
        assertTrue(archived.archived)
        assertNotNull(archived.archivedAt)
        // Visible at once in the flow, and nothing else about the item changed (read state, expiry, deliveries included).
        assertTrue(core.items.first().first { it.id == "demo-text-1" }.archived)
        assertEquals(before.copy(archived = true, archivedAt = archived.archivedAt), archived)

        core.unarchive("demo-text-1")
        assertEquals(before, core.getItem("demo-text-1"))
    }

    @Test
    fun archiveAndUnarchiveAreIdempotentAndIgnoreUnknownIds() = runTest {
        val core = core()
        core.archive("demo-text-1")
        val firstArchivedAt = core.getItem("demo-text-1")!!.archivedAt
        // A second archive changes nothing: same state, same timestamp.
        core.archive("demo-text-1")
        assertEquals(firstArchivedAt, core.getItem("demo-text-1")!!.archivedAt)
        // Unarchive of a non-archived item and calls on an unknown id are no-ops, not errors.
        val snapshot = core.items.first()
        core.unarchive("demo-image-1")
        core.archive("nope")
        core.unarchive("nope")
        assertEquals(snapshot, core.items.first())
        assertNull(core.getItem("nope"))
        // Twice in a row, then back: still consistent.
        core.unarchive("demo-text-1")
        core.unarchive("demo-text-1")
        assertFalse(core.getItem("demo-text-1")!!.archived)
    }

    @Test
    fun archivingNeverReordersTheFlowNorMarksReadNorTouchesExpiry() = runTest {
        val core = core()
        val idsBefore = core.items.first().map { it.id }
        val md = core.getItem("demo-md-1")!!
        assertFalse(md.read)
        core.archive("demo-md-1")
        assertEquals(idsBefore, core.items.first().map { it.id })
        val after = core.getItem("demo-md-1")!!
        assertFalse("archiving must not mark the item read", after.read)
        assertEquals(md.expiresAt, after.expiresAt)
        assertEquals(md.readPolicy, after.readPolicy)
    }

    @Test
    fun anUnreadAfterFirstReadSecretSurvivesBeingArchived() = runTest {
        val core = core()
        val secret = core.getItem("demo-archived-secret")!!
        assertEquals(ReadPolicy.AfterFirstRead, secret.readPolicy)
        assertTrue(secret.archived)
        assertFalse(secret.read)
        // The demo ships it archived, and un-archiving it (the swipe in Archives) does not consume it either.
        core.unarchive("demo-archived-secret")
        val back = core.getItem("demo-archived-secret")
        assertNotNull(back)
        assertFalse(back!!.read)
        core.archive("demo-archived-secret")
        assertFalse(core.getItem("demo-archived-secret")!!.read)
    }

    @Test
    fun aHiddenAndAnUnreadableItemCanBeArchived() = runTest {
        val core = core()
        core.archive("demo-sent-secret")
        core.archive("demo-unreadable-1")
        assertTrue(core.getItem("demo-sent-secret")!!.archived)
        assertTrue(core.getItem("demo-unreadable-1")!!.archived)
        assertTrue(core.getItem("demo-sent-secret")!!.secret)
        assertFalse(core.getItem("demo-unreadable-1")!!.isReadable)
    }

    // ---- Pin and archive exclude each other ------------------------------------------------------------------------

    @Test
    fun archivingAPinnedItemUnpinsIt() = runTest {
        val core = core()
        core.pin("demo-text-1")
        assertTrue(core.getItem("demo-text-1")!!.pinned)
        core.archive("demo-text-1")
        val item = core.getItem("demo-text-1")!!
        assertTrue(item.archived)
        assertFalse(item.pinned)
        assertNull(item.pinnedAt)
    }

    @Test
    fun pinningAnArchivedItemUnarchivesIt() = runTest {
        val core = core()
        core.archive("demo-text-1")
        core.pin("demo-text-1")
        val item = core.getItem("demo-text-1")!!
        assertTrue(item.pinned)
        assertNotNull(item.pinnedAt)
        assertFalse(item.archived)
        assertNull(item.archivedAt)
    }

    @Test
    fun noSequenceOfPinAndArchiveLeavesAnItemBothPinnedAndArchived() = runTest {
        val core = core()
        val calls: List<suspend () -> Unit> = listOf(
            { core.pin("demo-image-1") }, { core.archive("demo-image-1") }, { core.unpin("demo-image-1") },
            { core.unarchive("demo-image-1") }, { core.archive("demo-image-1") }, { core.pin("demo-image-1") },
            { core.pin("demo-image-1") }, { core.archive("demo-image-1") }, { core.archive("demo-image-1") },
        )
        for (call in calls) {
            call()
            val item = core.getItem("demo-image-1")!!
            assertFalse("pinned and archived at once", item.pinned && item.archived)
        }
    }

    @Test
    fun theDemoShipsArchivedItemsAndNoPinnedArchivedOne() = runTest {
        val items = core().items.first()
        val archived = items.filter { it.archived }
        assertTrue("the demo needs at least two archived items", archived.size >= 2)
        assertTrue(archived.none { it.pinned })
        assertTrue(archived.all { it.archivedAt != null })
    }

    // ---- Filtering ------------------------------------------------------------------------------------------------

    @Test
    fun archivedItemsAreAbsentFromEveryFilterButArchives() {
        val link = item("link", archived = true) { readableItem(it, office, base, url = "https://a.example") }
        val text = item("text", archived = true)
        val file = item("file", archived = true) { readableItem(it, office, base, attachments = listOf(Attachment("a.pdf", "application/pdf", 1))) }
        val secret = item("secret", archived = true) { readableItem(it, office, base, secret = true, text = RichText("s")) }
        val live = plain("live")
        val all = listOf(link, text, file, secret, live)

        for (filter in listOf(FeedFilter.All, FeedFilter.Links, FeedFilter.Text, FeedFilter.Files, FeedFilter.Secrets)) {
            assertTrue("$filter", FeedFiltering.apply(all, filter, "").none { it.archived })
        }
        assertEquals(listOf("live"), FeedFiltering.apply(all, FeedFilter.All, "").map { it.id })
        assertEquals(listOf("link", "text", "file", "secret"), FeedFiltering.apply(all, FeedFilter.Archives, "").map { it.id })
    }

    @Test
    fun theArchivesFilterOnlyShowsArchivedItems() {
        val all = listOf(plain("a"), item("b", archived = true), plain("c"))
        assertEquals(listOf("b"), FeedFiltering.apply(all, FeedFilter.Archives, "").map { it.id })
        assertTrue(FeedFiltering.apply(listOf(plain("a")), FeedFilter.Archives, "").isEmpty())
        // The search works inside Archives, the hidden-item rule too.
        assertEquals(listOf("b"), FeedFiltering.apply(all, FeedFilter.Archives, "text b").map { it.id })
        assertTrue(FeedFiltering.apply(all, FeedFilter.Archives, "text a").isEmpty())
        val secret = item("s", archived = true) { readableItem(it, office, base, secret = true, title = "Wi-Fi", text = RichText("hunter2")) }
        assertTrue(FeedFiltering.apply(listOf(secret), FeedFilter.Archives, "hunter2").isEmpty())
        assertEquals(listOf("s"), FeedFiltering.apply(listOf(secret), FeedFilter.Archives, "bureau").map { it.id })
    }

    @Test
    fun anArchivedItemStaysSearchableFromAllButOnlyWhileSearching() {
        val archived = item("old", archived = true) { readableItem(it, office, base, title = "Recette tarte") }
        val live = readableItem("live", office, base, title = "Recette soupe")
        val all = listOf(live, archived)
        // No query: archived items stay out of "All".
        assertEquals(listOf("live"), FeedFiltering.apply(all, FeedFilter.All, "").map { it.id })
        assertEquals(listOf("live"), FeedFiltering.apply(all, FeedFilter.All, "  ").map { it.id })
        // With a query: found, archive badge on the card.
        assertEquals(listOf("live", "old"), FeedFiltering.apply(all, FeedFilter.All, "recette").map { it.id })
        assertEquals(listOf("old"), FeedFiltering.apply(all, FeedFilter.All, "tarte").map { it.id })
        // A typed filter never lists archived items, query or not.
        val link = item("lnk", archived = true) { readableItem(it, office, base, url = "https://recette.example") }
        assertTrue(FeedFiltering.apply(listOf(link), FeedFilter.Links, "recette").isEmpty())
    }

    @Test
    fun anItemThatIsPinnedAndArchivedIsShownAsPinned() {
        // A core never reports both; if devices converge on both, the pin wins (docs section 2.3).
        val both = item("both", archived = true, pinned = true)
        assertFalse(both.isShownAsArchived())
        assertEquals(listOf("both"), FeedFiltering.apply(listOf(both), FeedFilter.All, "").map { it.id })
        assertTrue(FeedFiltering.apply(listOf(both), FeedFilter.Archives, "").isEmpty())
        assertEquals(listOf("both"), FeedSections.split(FeedFiltering.apply(listOf(both), FeedFilter.All, "")).pinned.map { it.id })
    }

    @Test
    fun archivedItemsLeaveBothSectionsOfTheFeed() {
        val items = listOf(plain("new"), item("arch", archived = true), item("pin", pinned = true), plain("old"))
        val sections = FeedSections.split(FeedFiltering.apply(items, FeedFilter.All, ""))
        assertEquals(listOf("pin"), sections.pinned.map { it.id })
        assertEquals(listOf("new", "old"), sections.recent.map { it.id })
    }

    @Test
    fun filteringReactsToTheCoreFlow() = runTest {
        val core = core()
        fun recentIds(items: List<TomiItem>) = FeedFiltering.apply(items, FeedFilter.All, "").map { it.id }
        assertTrue("demo-text-1" in recentIds(core.items.first()))
        core.archive("demo-text-1")
        assertFalse("demo-text-1" in recentIds(core.items.first()))
        assertTrue("demo-text-1" in FeedFiltering.apply(core.items.first(), FeedFilter.Archives, "").map { it.id })
        core.unarchive("demo-text-1")
        assertTrue("demo-text-1" in recentIds(core.items.first()))
    }

    // ---- Swipe dispatch -------------------------------------------------------------------------------------------

    private fun outgoing(id: String, vararg states: DeliveryState) = readableItem(
        id, local, base, text = RichText(id),
        deliveries = states.mapIndexed { i, s -> DeviceDelivery("d$i", "D$i", s) },
    )

    @Test
    fun theSwipeArchivesARecentCardAndUnarchivesAnArchivedOne() {
        val me = FakeTomiCore.LOCAL_ID
        assertEquals(ArchiveAction.Archive, ArchiveSwipe.swipeAction(plain("a"), me))
        assertEquals(ArchiveAction.Unarchive, ArchiveSwipe.swipeAction(item("b", archived = true), me))
        assertEquals(ArchiveAction.Archive, ArchiveSwipe.swipeAction(outgoing("sent", DeliveryState.Delivered, DeliveryState.Sent), me))
    }

    @Test
    fun aPinnedCardDoesNotSwipe() {
        assertNull(ArchiveSwipe.swipeAction(item("p", pinned = true), FakeTomiCore.LOCAL_ID))
        // The menu still offers "Archive" for it (archiving unpins).
        assertTrue(ArchiveSwipe.canArchive(item("p", pinned = true), FakeTomiCore.LOCAL_ID))
        assertEquals(ArchiveAction.Archive, ArchiveSwipe.menuAction(item("p", pinned = true)))
    }

    @Test
    fun aQueuedOrFailedOutgoingItemCannotBeArchived() {
        val me = FakeTomiCore.LOCAL_ID
        val queued = outgoing("q", DeliveryState.Queued, DeliveryState.Queued)
        val failed = outgoing("f", DeliveryState.Delivered, DeliveryState.Failed)
        assertNull(ArchiveSwipe.swipeAction(queued, me))
        assertNull(ArchiveSwipe.swipeAction(failed, me))
        assertFalse(ArchiveSwipe.canArchive(queued, me))
        assertFalse(ArchiveSwipe.canArchive(failed, me))
        // Someone else's queued delivery does not concern this device.
        assertTrue(ArchiveSwipe.canArchive(queued, "another-device"))
    }

    @Test
    fun theSwipeActionReachesTheCoreContractAndNothingElse() = runTest {
        val core = core()
        val me = core.localDevice.value.id
        val before = core.getItem("demo-text-1")!!

        val action = ArchiveSwipe.swipeAction(before, me)!!
        ArchiveSwipe.perform(core, action, before.id)
        assertTrue(core.getItem("demo-text-1")!!.archived)
        assertEquals(before.read, core.getItem("demo-text-1")!!.read)

        val back = ArchiveSwipe.swipeAction(core.getItem("demo-text-1")!!, me)
        assertEquals(ArchiveAction.Unarchive, back)
        ArchiveSwipe.perform(core, back!!, "demo-text-1")
        assertEquals(before, core.getItem("demo-text-1"))
    }

    @Test
    fun undoingAnArchiveRestoresThePinThatItRemoved() = runTest {
        val core = core()
        core.pin("demo-text-1")
        val pinned = core.getItem("demo-text-1")!!
        ArchiveSwipe.perform(core, ArchiveAction.Archive, pinned.id)
        assertFalse(core.getItem("demo-text-1")!!.pinned)

        ArchiveSwipe.undo(core, ArchiveAction.Archive, pinned.id, wasPinned = pinned.pinned)
        val restored = core.getItem("demo-text-1")!!
        assertFalse(restored.archived)
        assertTrue(restored.pinned)
    }

    @Test
    fun undoingAnArchiveOfAnUnpinnedItemJustUnarchives() = runTest {
        val core = core()
        ArchiveSwipe.perform(core, ArchiveAction.Archive, "demo-text-1")
        ArchiveSwipe.undo(core, ArchiveAction.Archive, "demo-text-1", wasPinned = false)
        val item = core.getItem("demo-text-1")!!
        assertFalse(item.archived)
        assertFalse(item.pinned)
    }

    @Test
    fun undoingAnUnarchiveArchivesAgain() = runTest {
        val core = core()
        ArchiveSwipe.perform(core, ArchiveAction.Unarchive, "demo-archived-link")
        assertFalse(core.getItem("demo-archived-link")!!.archived)
        ArchiveSwipe.undo(core, ArchiveAction.Unarchive, "demo-archived-link", wasPinned = false)
        assertTrue(core.getItem("demo-archived-link")!!.archived)
        assertEquals(ArchiveAction.Unarchive, ArchiveAction.Archive.inverse)
        assertEquals(ArchiveAction.Archive, ArchiveAction.Unarchive.inverse)
    }

    @Test
    fun theWholeCycleArchiveBannerUndoBringsTheItemBackToRecent() = runTest {
        val core = core()
        val me = core.localDevice.value.id
        suspend fun recentIds() = FeedSections.split(FeedFiltering.apply(core.items.first(), FeedFilter.All, "")).recent.map { it.id }
        val before = core.getItem("demo-text-1")!!
        assertTrue("demo-text-1" in recentIds())

        // Swipe (released past the threshold): the core archives, the card leaves Recent.
        val action = ArchiveSwipe.swipeAction(before, me)!!
        ArchiveSwipe.perform(core, action, before.id)
        assertFalse("demo-text-1" in recentIds())
        assertTrue(core.getItem("demo-text-1")!!.archived)

        // "Undo" of the banner: the inverse call, the card is back in Recent, not archived.
        ArchiveSwipe.undo(core, action, before.id, wasPinned = before.pinned)
        assertTrue("demo-text-1" in recentIds())
        assertFalse(core.getItem("demo-text-1")!!.archived)
        // And the swipe offered on it is "Archive" again (no stale state).
        assertEquals(ArchiveAction.Archive, ArchiveSwipe.swipeAction(core.getItem("demo-text-1")!!, me))
    }

    @Test
    fun undoingAnArchiveFromTheMenuOfAPinnedItemRepinsItAndLeavesRecent() = runTest {
        val core = core()
        core.pin("demo-text-1")
        val pinned = core.getItem("demo-text-1")!!
        ArchiveSwipe.perform(core, ArchiveSwipe.menuAction(pinned), pinned.id)
        ArchiveSwipe.undo(core, ArchiveAction.Archive, pinned.id, wasPinned = pinned.pinned)
        val sections = FeedSections.split(FeedFiltering.apply(core.items.first(), FeedFilter.All, ""))
        assertTrue(sections.pinned.any { it.id == "demo-text-1" })
        assertFalse(sections.recent.any { it.id == "demo-text-1" })
    }

    // ---- Wide HTML in a card ---------------------------------------------------------------------------------------

    @Test
    fun theCardHtmlDocumentCannotScrollSidewaysButTheDetailOneCan() {
        val card = HtmlSanitizer.toDocument("<table><tr><td>x</td></tr></table>", "#000000", "#0000FF", horizontalScroll = false)
        val detail = HtmlSanitizer.toDocument("<table><tr><td>x</td></tr></table>", "#000000", "#0000FF")
        assertTrue(card.contains("overflow-x: hidden"))
        assertFalse(detail.contains("overflow-x"))
    }
}
