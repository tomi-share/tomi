package eu.shaffe.tomi.ui.feed

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SeenTrackerTest {
    @Test
    fun aKeyIsDueOnlyAfterTheDwell() {
        val t = SeenTracker(dwellMs = 1_000)
        t.update(setOf("a"), now = 0)
        assertTrue(t.takeDue(now = 999).isEmpty())
        assertEquals(listOf("a"), t.takeDue(now = 1_000))
    }

    @Test
    fun aKeyIsReportedOnceUntilItLeavesTheScreen() {
        val t = SeenTracker(dwellMs = 1_000)
        t.update(setOf("a"), 0)
        assertEquals(listOf("a"), t.takeDue(1_000))
        assertTrue(t.takeDue(5_000).isEmpty())
        assertNull(t.nextDelay(5_000))
        t.update(emptySet(), 6_000)
        t.update(setOf("a"), 7_000)
        assertEquals(listOf("a"), t.takeDue(8_000))
    }

    @Test
    fun aQuickScrollPastReadsNothing() {
        val t = SeenTracker(dwellMs = 1_000)
        t.update(setOf("a"), 0)
        t.update(emptySet(), 400)
        assertTrue(t.takeDue(2_000).isEmpty())
        assertNull(t.nextDelay(2_000))
    }

    @Test
    fun eachKeyKeepsItsOwnStartWhenTheSetChanges() {
        val t = SeenTracker(dwellMs = 1_000)
        t.update(setOf("a"), 0)
        t.update(setOf("a", "b"), 600)
        assertEquals(400L, t.nextDelay(600))
        assertEquals(listOf("a"), t.takeDue(1_000))
        assertEquals(600L, t.nextDelay(1_000))
        assertEquals(listOf("b"), t.takeDue(1_600))
    }

    @Test
    fun resetForgetsTheTimeSpentWhileNotAttended() {
        val t = SeenTracker(dwellMs = 1_000)
        t.update(setOf("a"), 0)
        t.reset()
        t.update(setOf("a"), 900)
        assertTrue(t.takeDue(1_000).isEmpty())
        assertEquals(listOf("a"), t.takeDue(1_900))
    }

    @Test
    fun aCardNeedsMostOfItselfOnScreen() {
        // Free zone 0..1000, card of 200 px.
        assertTrue(SeenTracker.visibleEnough(offset = 0, size = 200, top = 0, bottom = 1000))
        assertTrue(SeenTracker.visibleEnough(offset = 880, size = 200, top = 0, bottom = 1000)) // 120/200 = 60 %
        assertFalse(SeenTracker.visibleEnough(offset = 900, size = 200, top = 0, bottom = 1000)) // 50 %
        assertFalse(SeenTracker.visibleEnough(offset = -100, size = 200, top = 0, bottom = 1000)) // 50 %
        assertFalse(SeenTracker.visibleEnough(offset = 1200, size = 200, top = 0, bottom = 1000))
    }

    @Test
    fun aCardTallerThanTheZoneNeedsMostOfTheZone() {
        // Card of 1500 px in a 1000 px zone: 600 px of it on screen is enough.
        assertTrue(SeenTracker.visibleEnough(offset = -900, size = 1500, top = 0, bottom = 1000))
        assertFalse(SeenTracker.visibleEnough(offset = -1000, size = 1500, top = 0, bottom = 1000))
    }
}
