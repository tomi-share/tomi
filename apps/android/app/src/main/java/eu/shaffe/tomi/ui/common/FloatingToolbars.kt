package eu.shaffe.tomi.ui.common

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.background
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FloatingToolbarDefaults
import androidx.compose.material3.HorizontalFloatingToolbar
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.ui.theme.TomiTheme

/** Margin between the floating toolbar and the bottom of the content area (above the navigation bar). */
val ToolbarBottomMargin = 16.dp

/** Breathing room kept between the last content and the top of the toolbar. */
private val ToolbarContentGap = 24.dp

/** Used until the toolbar has been measured (first frame), and as a floor. */
private val MinToolbarClearance = 112.dp

/**
 * Bottom space a scrolling screen must reserve so its last item stays fully visible above the floating toolbar:
 * measured toolbar height + [ToolbarBottomMargin] + a small gap. Create it with [rememberToolbarClearance], give the
 * toolbar `Modifier.measureAsToolbar(state)`, and use [bottomPadding] as the content's bottom padding.
 */
@Stable
class ToolbarClearanceState {
    var toolbarHeight by mutableStateOf(0.dp)
        internal set

    val bottomPadding: Dp
        get() = maxOf(MinToolbarClearance, toolbarHeight + ToolbarBottomMargin + ToolbarContentGap)
}

@Composable
fun rememberToolbarClearance(): ToolbarClearanceState = remember { ToolbarClearanceState() }

/** Reports the size of the toolbar this modifier is on to [state]. */
@Composable
fun Modifier.measureAsToolbar(state: ToolbarClearanceState): Modifier {
    val density = LocalDensity.current
    return this.onSizeChanged { size -> state.toolbarHeight = with(density) { size.height.toDp() } }
}

/**
 * Material 3 Expressive floating toolbar with the main action as an extended FAB attached on the right (thumb side).
 * Without [fab] the toolbar stands alone. It stays in place while the content scrolls (hiding it on scroll felt odd).
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun TomiFloatingToolbar(
    modifier: Modifier = Modifier,
    fab: (@Composable () -> Unit)? = null,
    content: @Composable RowScope.() -> Unit,
) {
    val colors = FloatingToolbarDefaults.standardFloatingToolbarColors(
        toolbarContainerColor = toolbarContainerColor(),
        toolbarContentColor = MaterialTheme.colorScheme.onSurface,
    )
    if (fab != null) {
        HorizontalFloatingToolbar(
            expanded = true,
            floatingActionButton = fab,
            modifier = modifier,
            colors = colors,
            content = content,
        )
    } else {
        HorizontalFloatingToolbar(expanded = true, modifier = modifier, colors = colors, content = content)
    }
}

/**
 * Container of the toolbar. The default (`surfaceContainer`) equals the card colour in dark (#1A1F2B on #121620), so the
 * toolbar melted into the feed and its icons were hard to see: it floats, so it must stand out from what scrolls under it.
 * Light: white cards on a beige background leave no neutral that stands out, so the container is a clearly blue-tinted
 * brand tone (about #C2CDDF). Dark: shadows are invisible, so the container is lifted toward the brand colour (about
 * #414B5F). Both read against the background and the cards, with the icons and the FAB still well above 4.5:1.
 */
@Composable
private fun toolbarContainerColor(): Color {
    val scheme = MaterialTheme.colorScheme
    val dark = scheme.background.luminance() < 0.5f
    return if (dark) {
        lerp(scheme.surfaceContainerHighest, scheme.primary, 0.22f)
    } else {
        lerp(scheme.primaryContainer, scheme.primary, 0.18f)
    }
}

/**
 * THE single "main action" button of Tomi (Send in the feed and the composer, Copy in the detail), attached to the
 * toolbar. One style everywhere, whatever the radii variant (charter / Material 3 Expressive):
 *
 * - square with the charter FAB radius ([TomiTheme.shapes.fab] is 12 dp in both variants);
 * - solid `color.*.brand` (`colorScheme.primary`: #2E4C80 light, #A9BFEA dark) with the icon in `color.*.on-brand`
 *   (`colorScheme.onPrimary`: #FFFFFF light, #121620 dark), not the tinted brand-container;
 * - icon only: the floating toolbar owns the size of its FAB slot (a square), so the FAB must fill it
 *   (`FloatingToolbarDefaults.VibrantFloatingActionButton` applies `fillMaxSize`) and the icon is centred in it.
 *   An extended FAB (icon + label) does not fit that square and pushed the icon off-centre. The label stays as the
 *   accessibility description.
 *
 * A disabled FAB is dimmed and ignores taps.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ToolbarFab(
    contentDescription: String,
    icon: ImageVector,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val container = MaterialTheme.colorScheme.primary
    FloatingToolbarDefaults.VibrantFloatingActionButton(
        onClick = { if (enabled) onClick() },
        modifier = modifier.semantics { if (!enabled) disabled() },
        shape = TomiTheme.shapes.fab,
        containerColor = if (enabled) container else container.copy(alpha = 0.38f),
        contentColor = MaterialTheme.colorScheme.onPrimary,
    ) {
        // Icon() centres its glyph in the FAB's Box; the glyph is a 24 dp icon, so no extra offset is needed.
        Icon(icon, contentDescription = contentDescription, modifier = Modifier.size(24.dp))
    }
}

/** Icon button of the toolbar: [selected] gives it the tinted container, [badge] a small dot (active filter). */
@Composable
fun ToolbarIconButton(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    selected: Boolean = false,
    badge: Boolean = false,
    tint: Color = Color.Unspecified,
) {
    IconButton(
        onClick = onClick,
        modifier = modifier,
        colors = IconButtonDefaults.iconButtonColors(
            // Translucent brand tint: stays visible on the toolbar container in light and dark (primaryContainer is too
            // close to the dark toolbar colour).
            containerColor = if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.24f) else Color.Transparent,
            contentColor = if (tint != Color.Unspecified) tint else MaterialTheme.colorScheme.onSurface,
        ),
    ) {
        Box {
            Icon(icon, contentDescription = contentDescription)
            if (badge) {
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .offset(x = 3.dp, y = (-3).dp)
                        .size(8.dp)
                        .clip(CircleShape)
                        .background(MaterialTheme.colorScheme.primary),
                )
            }
        }
    }
}
