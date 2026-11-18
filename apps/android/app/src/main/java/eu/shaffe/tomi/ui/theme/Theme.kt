package eu.shaffe.tomi.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.BuildConfig

/**
 * Where the corner-radius variant is chosen.
 *
 * - `false` (default): radii of the Tomi charter (cards, buttons, fields 8 dp; sheets, FAB, menus 12 dp).
 * - `true`: "Material 3 Expressive" variant, rounder, for visual comparison only.
 *
 * The initial value comes from the build flag `EXPRESSIVE_RADII` (gradle property `-PtomiExpressiveRadii=true`).
 */
object TomiDesign {
    var expressiveRadii: Boolean by mutableStateOf(BuildConfig.EXPRESSIVE_RADII)
}

/** Extra colours of the charter that Material's [ColorScheme] has no slot for. */
@Immutable
class TomiColors(
    /** Plum: marks the hidden ("Masqué") items. */
    val secret: Color,
    val secretContainer: Color,
    val success: Color,
    /** Ochre: waiting states (offline, queued), search highlight. */
    val warning: Color,
)

/** Shapes used by the app components. Material's own [Shapes] are mapped from them too. */
@Immutable
class TomiShapes(
    val card: Shape,
    val button: Shape,
    val field: Shape,
    val maskBlock: Shape,
    val sheet: Shape,
    val fab: Shape,
    val menu: Shape,
    val thumbnail: Shape,
)

private val CharterShapes = TomiShapes(
    card = RoundedCornerShape(8.dp),
    button = RoundedCornerShape(8.dp),
    field = RoundedCornerShape(8.dp),
    maskBlock = RoundedCornerShape(8.dp),
    sheet = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp),
    fab = RoundedCornerShape(12.dp),
    menu = RoundedCornerShape(12.dp),
    thumbnail = RoundedCornerShape(8.dp),
)

private val ExpressiveShapes = TomiShapes(
    card = RoundedCornerShape(20.dp),
    button = CircleShape,
    field = RoundedCornerShape(16.dp),
    maskBlock = RoundedCornerShape(16.dp),
    sheet = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    // Square like the charter: the toolbar FAB has ONE style whatever the radii variant (design choice).
    fab = RoundedCornerShape(12.dp),
    menu = RoundedCornerShape(20.dp),
    thumbnail = RoundedCornerShape(16.dp),
)

private val LocalTomiColors = staticCompositionLocalOf<TomiColors> { error("TomiTheme missing") }
private val LocalTomiShapes = staticCompositionLocalOf<TomiShapes> { error("TomiTheme missing") }

object TomiTheme {
    val colors: TomiColors @Composable @ReadOnlyComposable get() = LocalTomiColors.current
    val shapes: TomiShapes @Composable @ReadOnlyComposable get() = LocalTomiShapes.current
}

// Values of design/tomi/tokens.json (source of truth). Light mode.
internal val LightScheme = lightColorScheme(
    primary = Color(0xFF2E4C80),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFE3E9F4),
    onPrimaryContainer = Color(0xFF1B2130),
    secondary = Color(0xFF8C5F14),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFE3E9F4),
    onSecondaryContainer = Color(0xFF1B2130),
    tertiary = Color(0xFF6F4F7C),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFEFE8F1),
    onTertiaryContainer = Color(0xFF6F4F7C),
    background = Color(0xFFF6F4EF),
    onBackground = Color(0xFF1B2130),
    surface = Color(0xFFFFFFFF),
    onSurface = Color(0xFF1B2130),
    surfaceVariant = Color(0xFFECE9E2),
    onSurfaceVariant = Color(0xFF565C6B),
    surfaceTint = Color(0xFF2E4C80),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFFFFFFF),
    surfaceContainer = Color(0xFFFFFFFF),
    surfaceContainerHigh = Color(0xFFECE9E2),
    surfaceContainerHighest = Color(0xFFECE9E2),
    surfaceDim = Color(0xFFECE9E2),
    surfaceBright = Color(0xFFFFFFFF),
    outline = Color(0xFF858996),
    outlineVariant = Color(0xFFD5D1C7),
    error = Color(0xFFB3261E),
    onError = Color(0xFFFFFFFF),
    // No token for this role: a discreet red tint, deliberately DISTINCT from every other role. Material's contentColorFor
    // compares by VALUE and tests errorContainer before surfaceVariant / surfaceContainerHigh(est) / surfaceBright /
    // surfaceDim: with the same value as them, every Surface on those colours inherited onErrorContainer (red text).
    // onErrorContainer on it: 5.14:1. Guarded by ThemeContentColorTest.
    errorContainer = Color(0xFFF9DEDC),
    onErrorContainer = Color(0xFFB3261E),
    inverseSurface = Color(0xFF1B2130),
    inverseOnSurface = Color(0xFFF6F4EF),
    inversePrimary = Color(0xFFA9BFEA),
)

private val LightExtras = TomiColors(
    secret = Color(0xFF6F4F7C),
    secretContainer = Color(0xFFEFE8F1),
    success = Color(0xFF2E6B45),
    warning = Color(0xFF8C5F14),
)

// Dark mode.
internal val DarkScheme = darkColorScheme(
    primary = Color(0xFFA9BFEA),
    onPrimary = Color(0xFF121620),
    primaryContainer = Color(0xFF24324A),
    onPrimaryContainer = Color(0xFFE9E7E1),
    secondary = Color(0xFFE0B160),
    onSecondary = Color(0xFF121620),
    secondaryContainer = Color(0xFF24324A),
    onSecondaryContainer = Color(0xFFE9E7E1),
    tertiary = Color(0xFFCDB3D9),
    onTertiary = Color(0xFF121620),
    tertiaryContainer = Color(0xFF2E2636),
    onTertiaryContainer = Color(0xFFCDB3D9),
    background = Color(0xFF121620),
    onBackground = Color(0xFFE9E7E1),
    surface = Color(0xFF1A1F2B),
    onSurface = Color(0xFFE9E7E1),
    surfaceVariant = Color(0xFF242A38),
    onSurfaceVariant = Color(0xFFA6ABB8),
    surfaceTint = Color(0xFFA9BFEA),
    surfaceContainerLowest = Color(0xFF1A1F2B),
    surfaceContainerLow = Color(0xFF1A1F2B),
    surfaceContainer = Color(0xFF1A1F2B),
    surfaceContainerHigh = Color(0xFF242A38),
    surfaceContainerHighest = Color(0xFF242A38),
    surfaceDim = Color(0xFF121620),
    surfaceBright = Color(0xFF242A38),
    outline = Color(0xFF6F7587),
    outlineVariant = Color(0xFF383E4C),
    error = Color(0xFFF2B8B5),
    onError = Color(0xFF121620),
    // Discreet red tint, distinct from every other role (see the light scheme: contentColorFor compares by value).
    // onErrorContainer on it: 8.49:1.
    errorContainer = Color(0xFF3A2326),
    onErrorContainer = Color(0xFFF2B8B5),
    inverseSurface = Color(0xFFE9E7E1),
    inverseOnSurface = Color(0xFF121620),
    inversePrimary = Color(0xFF2E4C80),
)

private val DarkExtras = TomiColors(
    secret = Color(0xFFCDB3D9),
    secretContainer = Color(0xFF2E2636),
    success = Color(0xFF8FCFA6),
    warning = Color(0xFFE0B160),
)

private fun TomiShapes.toMaterial(radii: MaterialRadii): Shapes = Shapes(
    extraSmall = RoundedCornerShape(4.dp),
    small = RoundedCornerShape(radii.field),
    medium = RoundedCornerShape(radii.card),
    large = RoundedCornerShape(radii.menu),
    extraLarge = RoundedCornerShape(radii.fab),
)

/** Radii (dp) behind the Material [Shapes] slots, which need corner-based shapes. */
private class MaterialRadii(val field: androidx.compose.ui.unit.Dp, val card: androidx.compose.ui.unit.Dp, val menu: androidx.compose.ui.unit.Dp, val fab: androidx.compose.ui.unit.Dp)

private val CharterRadii = MaterialRadii(8.dp, 8.dp, 12.dp, 12.dp)
private val ExpressiveRadii = MaterialRadii(16.dp, 20.dp, 20.dp, 28.dp)

/** Material 3 with the colours, typography and radii of the Tomi charter (static colours, no dynamic colour). */
@Composable
fun TomiTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    expressiveRadii: Boolean = TomiDesign.expressiveRadii,
    content: @Composable () -> Unit,
) {
    val shapes = if (expressiveRadii) ExpressiveShapes else CharterShapes
    CompositionLocalProvider(
        LocalTomiColors provides (if (darkTheme) DarkExtras else LightExtras),
        LocalTomiShapes provides shapes,
    ) {
        MaterialTheme(
            colorScheme = if (darkTheme) DarkScheme else LightScheme,
            typography = TomiTypography,
            shapes = shapes.toMaterial(if (expressiveRadii) ExpressiveRadii else CharterRadii),
            content = content,
        )
    }
}
