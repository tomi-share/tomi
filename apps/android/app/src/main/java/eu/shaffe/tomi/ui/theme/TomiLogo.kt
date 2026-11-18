package eu.shaffe.tomi.ui.theme

import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.height
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.graphics.vector.group
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

private const val LOGO_WIDTH = 634.4f
private const val LOGO_HEIGHT = 173.2f

private fun ImageVector.Builder.shape(data: String, evenOdd: Boolean = false) {
    addPath(
        pathData = PathParser().parsePathString(data).toNodes(),
        pathFillType = if (evenOdd) PathFillType.EvenOdd else PathFillType.NonZero,
        fill = SolidColor(Color.Black),
    )
}

/** Horizontal logo "symbol + tomi" (design/tomi/logo-horizontal.svg), drawn as a vector so it follows the text colour. */
private val TomiWordmark: ImageVector by lazy {
    ImageVector.Builder(
        name = "tomi",
        defaultWidth = LOGO_WIDTH.dp,
        defaultHeight = LOGO_HEIGHT.dp,
        viewportWidth = LOGO_WIDTH,
        viewportHeight = LOGO_HEIGHT,
    ).apply {
        // The SVG view box starts at y = 52.
        group(translationY = -52f) {
            group(translationX = -17.6f, translationY = 47.6f, scaleX = 0.8f, scaleY = 0.8f) {
                shape("M61.93 70.13A94 94 0 1 1 61.93 185.87L80.84 171.1A70 70 0 1 0 80.84 84.9Z")
                shape("M22 128A32 32 0 1 0 86 128A32 32 0 1 0 22 128Z")
            }
            group(translationX = 222.4f) {
                shape("M14 62V100H0V122H14V164A36 36 0 0 0 50 200H66V178H50A14 14 0 0 1 36 164V122H62V100H36V62Z")
                shape("M86 150A52 52 0 1 0 190 150A52 52 0 1 0 86 150Z M108 150A30 30 0 1 1 168 150A30 30 0 1 1 108 150Z", evenOdd = true)
                shape("M212 200V100H234V105.89A44 44 0 0 1 289 114.9A44 44 0 0 1 366 144V200H344V144A22 22 0 0 0 300 144V200H278V144A22 22 0 0 0 234 144V200Z")
                shape("M390 100H412V200H390Z")
                shape("M387 66A14 14 0 1 0 415 66A14 14 0 1 0 387 66Z")
            }
        }
    }.build()
}

@Composable
fun TomiLogo(modifier: Modifier = Modifier, height: Dp = 28.dp, tint: Color = LocalContentColor.current) {
    Icon(
        imageVector = TomiWordmark,
        contentDescription = "Tomi",
        tint = tint,
        modifier = modifier.height(height).aspectRatio(LOGO_WIDTH / LOGO_HEIGHT),
    )
}
