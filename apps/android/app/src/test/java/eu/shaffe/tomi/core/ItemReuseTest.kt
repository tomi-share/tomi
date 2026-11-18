package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertSame
import org.junit.Test
import java.time.Instant

class ItemReuseTest {
    private val office = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-11T10:00:00Z")

    /** A new instance of everything at each call, as a new read of the core gives. */
    private fun note(id: String, text: String = "contenu de $id", read: Boolean = false, thumbnail: Byte? = null) = TomiItem(
        id = id,
        content = ItemContent.Readable(
            title = String("titre".toCharArray()),
            text = RichText(String(text.toCharArray()), markdown = String(text.toCharArray())),
            attachments = thumbnail?.let { listOf(Attachment("photo.jpg", "image/jpeg", 3, byteArrayOf(it, 2, 3))) } ?: emptyList(),
        ),
        sourceDevice = office,
        createdAt = now,
        read = read,
    )

    @Test
    fun anUnchangedItemKeepsItsInstance() {
        val previous = listOf(note("a"), note("b", thumbnail = 1))
        val fresh = listOf(note("a"), note("b", thumbnail = 1))
        val result = reuseUnchanged(previous, fresh)
        assertEquals(fresh, result)
        assertSame(previous[0], result[0])
        assertSame(previous[1], result[1])
    }

    @Test
    fun aChangeOfMetadataKeepsTheInstanceOfTheContent() {
        val previous = listOf(note("a", thumbnail = 1))
        val fresh = listOf(note("a", read = true, thumbnail = 1))
        val result = reuseUnchanged(previous, fresh).single()
        assertEquals(fresh.single(), result)
        assertNotSame(previous.single(), result)
        assertSame(previous.single().content, result.content)
        // The thumbnail is the same array: what was decoded from it stays valid.
        assertSame(previous.single().attachments.single().thumbnail, result.attachments.single().thumbnail)
    }

    @Test
    fun aChangedContentIsTheFreshOne() {
        val previous = listOf(note("a", text = "avant"), note("b", thumbnail = 1))
        val fresh = listOf(note("a", text = "après"), note("b", thumbnail = 9))
        val result = reuseUnchanged(previous, fresh)
        assertSame(fresh[0], result[0])
        assertSame(fresh[1], result[1])
    }

    @Test
    fun theResultFollowsTheFreshReadOnly() {
        val previous = listOf(note("gone"), note("kept"))
        val fresh = listOf(note("new"), note("kept"))
        val result = reuseUnchanged(previous, fresh)
        assertEquals(listOf("new", "kept"), result.map { it.id })
        assertSame(fresh[0], result[0])
        assertSame(previous[1], result[1])
    }

    @Test
    fun unreadableItemsAndEmptyReadsAreHandled() {
        fun unreadable() = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), office, now)
        val previous = listOf(unreadable())
        assertSame(previous.single(), reuseUnchanged(previous, listOf(unreadable())).single())
        assertEquals(emptyList<TomiItem>(), reuseUnchanged(previous, emptyList()))
        val fresh = listOf(note("a"))
        assertSame(fresh, reuseUnchanged(emptyList(), fresh))
    }
}
