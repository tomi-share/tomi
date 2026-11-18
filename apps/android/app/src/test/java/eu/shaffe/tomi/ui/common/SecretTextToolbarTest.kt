package eu.shaffe.tomi.ui.common

import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.TextToolbar
import androidx.compose.ui.platform.TextToolbarStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The selection toolbar wrapper of a hidden item: what it hands to the real system toolbar. It only covers the floating
 * selection bar of the Compose text fields, not a hardware Ctrl+C nor the "Copy" of a keyboard's own edit panel.
 */
class SecretTextToolbarTest {

    private class RecordingToolbar : TextToolbar {
        var shown = 0
        var copy: (() -> Unit)? = null
        var paste: (() -> Unit)? = null
        var cut: (() -> Unit)? = null
        var selectAll: (() -> Unit)? = null
        var hidden = 0
        override val status: TextToolbarStatus = TextToolbarStatus.Hidden

        override fun hide() {
            hidden++
        }

        override fun showMenu(
            rect: Rect,
            onCopyRequested: (() -> Unit)?,
            onPasteRequested: (() -> Unit)?,
            onCutRequested: (() -> Unit)?,
            onSelectAllRequested: (() -> Unit)?,
        ) {
            shown++
            copy = onCopyRequested
            paste = onPasteRequested
            cut = onCutRequested
            selectAll = onSelectAllRequested
        }
    }

    @Test
    fun copyAndCutAreDroppedAndPasteAndSelectAllAreKept() {
        val real = RecordingToolbar()
        NoCopyTextToolbar(real).showMenu(Rect.Zero, onCopyRequested = {}, onPasteRequested = {}, onCutRequested = {}, onSelectAllRequested = {})
        assertEquals(1, real.shown)
        assertNull(real.copy)
        assertNull(real.cut)
        assertNotNull(real.paste)
        assertNotNull(real.selectAll)
    }

    @Test
    fun theKeptActionsAreTheOriginalOnes() {
        val real = RecordingToolbar()
        var pasted = 0
        var selected = 0
        NoCopyTextToolbar(real).showMenu(Rect.Zero, onCopyRequested = {}, onPasteRequested = { pasted++ }, onCutRequested = {}, onSelectAllRequested = { selected++ })
        real.paste!!.invoke()
        real.selectAll!!.invoke()
        assertEquals(1, pasted)
        assertEquals(1, selected)
    }

    @Test
    fun hideAndStatusGoThroughToTheRealToolbar() {
        val real = RecordingToolbar()
        val wrapped = NoCopyTextToolbar(real)
        wrapped.hide()
        assertEquals(1, real.hidden)
        assertEquals(TextToolbarStatus.Hidden, wrapped.status)
    }
}
