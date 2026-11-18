package eu.shaffe.tomi.core

import eu.shaffe.tomi.ui.feed.FeedFilter
import eu.shaffe.tomi.ui.feed.FeedFiltering
import eu.shaffe.tomi.ui.feed.FeedSections
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

/** Pin / unpin as promised by spec/pin-contract.md, on the fake core, and the "Pinned" section built by the UI. */
@OptIn(ExperimentalCoroutinesApi::class)
class PinTest {
    private val office = DeviceRef("pc", "PC-Bureau")
    private val base = Instant.parse("2026-11-04T10:00:00Z")

    /** Clock that moves by one minute at each reading, so two pins never share a timestamp. */
    private class SteppingClock(private var now: Instant) : Clock() {
        override fun getZone() = ZoneOffset.UTC
        override fun withZone(zone: java.time.ZoneId?) = this
        override fun instant(): Instant = now.also { now = now.plus(Duration.ofMinutes(1)) }
    }

    private fun core(scope: CoroutineScope) = FakeTomiCore(scope, clock = SteppingClock(base), startPaired = true)

    @Test
    fun pinSetsPinnedAndPinnedAtAndUnpinClearsThem() = runTest {
        val core = core(CoroutineScope(Job()))
        val before = core.getItem("demo-text-1")!!
        assertFalse(before.pinned)
        assertNull(before.pinnedAt)

        core.pin("demo-text-1")
        val pinned = core.getItem("demo-text-1")!!
        assertTrue(pinned.pinned)
        assertNotNull(pinned.pinnedAt)
        // Visible at once in the flow, and nothing else about the item changed.
        assertTrue(core.items.first().first { it.id == "demo-text-1" }.pinned)
        assertEquals(before.copy(pinned = true, pinnedAt = pinned.pinnedAt), pinned)

        core.unpin("demo-text-1")
        val unpinned = core.getItem("demo-text-1")!!
        assertFalse(unpinned.pinned)
        assertNull(unpinned.pinnedAt)
    }

    @Test
    fun pinAndUnpinAreIdempotentAndIgnoreUnknownIds() = runTest {
        val core = core(CoroutineScope(Job()))
        core.pin("demo-text-1")
        val firstPinnedAt = core.getItem("demo-text-1")!!.pinnedAt
        // A second pin changes nothing: same state, same timestamp.
        core.pin("demo-text-1")
        assertEquals(firstPinnedAt, core.getItem("demo-text-1")!!.pinnedAt)
        // Unpin of a non-pinned item and calls on an unknown id are no-ops, not errors.
        core.unpin("demo-image-1")
        assertFalse(core.getItem("demo-image-1")!!.pinned)
        core.pin("nope")
        core.unpin("nope")
        assertNull(core.getItem("nope"))
    }

    @Test
    fun pinningNeverReordersTheCoreFlowNorTouchesReadOrExpiry() = runTest {
        val core = core(CoroutineScope(Job()))
        val idsBefore = core.items.first().map { it.id }
        val md = core.getItem("demo-md-1")!!
        core.pin("demo-md-1")
        assertEquals(idsBefore, core.items.first().map { it.id })
        val after = core.getItem("demo-md-1")!!
        assertEquals(md.expiresAt, after.expiresAt)
        assertEquals(md.readPolicy, after.readPolicy)
        assertEquals(md.read, after.read)
    }

    @Test
    fun aHiddenAndAnUnreadableItemCanBePinned() = runTest {
        val core = core(CoroutineScope(Job()))
        core.pin("demo-sent-secret")
        core.pin("demo-unreadable-1")
        assertTrue(core.getItem("demo-sent-secret")!!.pinned)
        assertTrue(core.getItem("demo-unreadable-1")!!.pinned)
        // Still hidden / still unreadable.
        assertTrue(core.getItem("demo-sent-secret")!!.secret)
        assertFalse(core.getItem("demo-unreadable-1")!!.isReadable)
    }

    @Test
    fun theSectionsPutPinnedItemsFirstMostRecentlyPinnedFirst() {
        fun item(id: String, minutes: Long, pinnedAtMinutes: Long? = null) = readableItem(
            id = id, sourceDevice = office, createdAt = base.plus(Duration.ofMinutes(minutes)), text = RichText(id),
            pinned = pinnedAtMinutes != null, pinnedAt = pinnedAtMinutes?.let { base.plus(Duration.ofMinutes(it)) },
        )
        // Newest first, as the core delivers them.
        val items = listOf(item("new", 50), item("pinned-old", 10, pinnedAtMinutes = 5), item("mid", 30), item("pinned-new", 5, pinnedAtMinutes = 60))
        val sections = FeedSections.split(items)
        assertEquals(listOf("pinned-new", "pinned-old"), sections.pinned.map { it.id })
        assertEquals(listOf("new", "mid"), sections.recent.map { it.id })
        assertFalse(sections.isEmpty)
        assertTrue(FeedSections.split(emptyList()).isEmpty)
    }

    @Test
    fun filteringAndSearchApplyInsideTheSections() {
        val link = readableItem("l", office, base, url = "https://a.example", pinned = true, pinnedAt = base)
        val text = readableItem("t", office, base, text = RichText("hello"))
        val sections = FeedSections.split(FeedFiltering.apply(listOf(link, text), FeedFilter.Links, ""))
        assertEquals(listOf("l"), sections.pinned.map { it.id })
        assertTrue(sections.recent.isEmpty())
    }
}
