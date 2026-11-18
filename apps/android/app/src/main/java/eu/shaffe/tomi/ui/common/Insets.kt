package eu.shaffe.tomi.ui.common

import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.Arrangement

/*
 * Edge-to-edge strategy (MainActivity and ShareActivity call enableEdgeToEdge(), the app draws behind both system bars).
 *
 * Every screen is a Material 3 Scaffold that uses [tomiContentInsets] as `contentWindowInsets`. `safeDrawing` is the union of
 * the status bar, the navigation / gesture bar, the display cutout and the soft keyboard (IME), so the content area handed
 * to the screen never goes under a system bar and always sits above the keyboard.
 *
 * The screen then applies the Scaffold padding with [screenContentPadding], which also CONSUMES those insets: any
 * `navigationBarsPadding()` / `imePadding()` placed further down (floating toolbar, text fields) is then a harmless no-op
 * instead of padding twice. Screens with a TopAppBar let the bar draw behind the status bar by itself (its own insets).
 */

/** Window insets every Tomi Scaffold uses for its content: status bar, navigation bar, cutout and keyboard. */
val tomiContentInsets: WindowInsets
    @Composable get() = WindowInsets.safeDrawing

/** Applies the Scaffold padding and marks those insets as consumed for the children. */
fun Modifier.screenContentPadding(padding: PaddingValues): Modifier =
    this.padding(padding).consumeWindowInsets(padding)

/**
 * A scrolling column whose content is centred vertically when it is shorter than the viewport (large phones), and
 * scrolls normally when it is taller. `Arrangement.Center` alone does nothing inside `verticalScroll` because the
 * height constraint is unbounded there, hence the `heightIn(min = viewport)`.
 */
@Composable
fun CenteredScrollColumn(
    modifier: Modifier = Modifier,
    verticalArrangement: Arrangement.Vertical = Arrangement.Center,
    horizontalAlignment: Alignment.Horizontal = Alignment.Start,
    content: @Composable ColumnScope.() -> Unit,
) {
    BoxWithConstraints(modifier) {
        val viewportHeight = maxHeight
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .heightIn(min = viewportHeight),
            verticalArrangement = verticalArrangement,
            horizontalAlignment = horizontalAlignment,
            content = content,
        )
    }
}
