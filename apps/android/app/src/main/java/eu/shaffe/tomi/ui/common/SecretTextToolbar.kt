package eu.shaffe.tomi.ui.common

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.LocalTextToolbar
import androidx.compose.ui.platform.TextToolbar
import androidx.compose.ui.platform.TextToolbarStatus

/**
 * The selection toolbar of the system, without "Copy" and "Cut": what is selected in a hidden item must not reach the
 * ordinary clipboard (no sensitive flag, no clearing after 30 s). "Paste" and "Select all" stay, the user can still write.
 */
internal class NoCopyTextToolbar(private val delegate: TextToolbar) : TextToolbar {
    override val status: TextToolbarStatus get() = delegate.status

    override fun hide() = delegate.hide()

    override fun showMenu(
        rect: Rect,
        onCopyRequested: (() -> Unit)?,
        onPasteRequested: (() -> Unit)?,
        onCutRequested: (() -> Unit)?,
        onSelectAllRequested: (() -> Unit)?,
    ) = delegate.showMenu(
        rect = rect,
        onCopyRequested = null,
        onPasteRequested = onPasteRequested,
        onCutRequested = null,
        onSelectAllRequested = onSelectAllRequested,
    )
}

/** Shows [content] with a selection toolbar that offers no copy or cut when [noCopy] is true; unchanged otherwise. */
@Composable
fun WithoutCopyActions(noCopy: Boolean, content: @Composable () -> Unit) {
    val current = LocalTextToolbar.current
    val toolbar = remember(current, noCopy) { if (noCopy) NoCopyTextToolbar(current) else current }
    CompositionLocalProvider(LocalTextToolbar provides toolbar, content = content)
}
