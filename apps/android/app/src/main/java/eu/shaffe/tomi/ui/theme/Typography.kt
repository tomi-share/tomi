package eu.shaffe.tomi.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import eu.shaffe.tomi.R

/** Fonts of the charter, embedded as variable fonts (SIL OFL 1.1): Inter for the UI, JetBrains Mono for code. */
object TomiFonts {
    @OptIn(ExperimentalTextApi::class)
    private fun inter(weight: FontWeight) = Font(
        R.font.inter_variable,
        weight = weight,
        variationSettings = FontVariation.Settings(FontVariation.weight(weight.weight)),
    )

    @OptIn(ExperimentalTextApi::class)
    private fun mono(weight: FontWeight) = Font(
        R.font.jetbrains_mono_variable,
        weight = weight,
        variationSettings = FontVariation.Settings(FontVariation.weight(weight.weight)),
    )

    val ui: FontFamily = FontFamily(inter(FontWeight.Normal), inter(FontWeight.Medium), inter(FontWeight.SemiBold))
    val mono: FontFamily = FontFamily(mono(FontWeight.Normal), mono(FontWeight.Medium), mono(FontWeight.SemiBold))

    /** Zero with a slash for revealed passwords (font.feature.mono-password) and tabular digits (font.feature.ui-digits). */
    const val MONO_PASSWORD_FEATURES = "zero"
    const val TABULAR_DIGITS = "tnum"

    /**
     * No contextual ligatures: JetBrains Mono draws `://` (and `->`, `==`...) through its `calt` feature, which showed
     * `https://` as `https: //`. A URL, a secret, a token or a command must be read EXACTLY as it is.
     * Android passes the string to HarfBuzz (`hb_feature_from_string`) piece by piece, split on commas: `tag=0` form,
     * no spaces. `liga` is off too, in case a future version of the font moves its ligatures there.
     */
    const val NO_LIGATURES = "calt=0,liga=0"

    /** Exact monospace text (URL, token, command): no ligature. */
    const val MONO_EXACT_FEATURES = NO_LIGATURES

    /** Revealed secret: slashed zero AND no ligature (several settings are combined with a comma). */
    const val MONO_SECRET_FEATURES = "$MONO_PASSWORD_FEATURES,$NO_LIGATURES"
}

private fun style(size: Int, line: Int, weight: FontWeight, family: FontFamily = TomiFonts.ui) = TextStyle(
    fontFamily = family,
    fontSize = size.sp,
    lineHeight = line.sp,
    fontWeight = weight,
)

// Scale of design/tomi/tokens.json: title 20/26, subtitle 16/22, body 15/22, label 14/20, caption 12/16, mono 14/20.
private val Title = style(20, 26, FontWeight.SemiBold)
private val Subtitle = style(16, 22, FontWeight.SemiBold)
private val Body = style(15, 22, FontWeight.Normal)
private val Label = style(14, 20, FontWeight.Medium)
private val Caption = style(12, 16, FontWeight.Normal)

private val Base = Typography()

val TomiTypography = Typography(
    displayLarge = Base.displayLarge.copy(fontFamily = TomiFonts.ui),
    displayMedium = Base.displayMedium.copy(fontFamily = TomiFonts.ui),
    displaySmall = Base.displaySmall.copy(fontFamily = TomiFonts.ui),
    headlineLarge = Title,
    headlineMedium = Title,
    headlineSmall = Title,
    titleLarge = Title,
    titleMedium = Subtitle,
    titleSmall = Label,
    bodyLarge = Body,
    bodyMedium = Body,
    bodySmall = Caption,
    labelLarge = Label,
    labelMedium = style(12, 16, FontWeight.Medium),
    labelSmall = Caption,
)
