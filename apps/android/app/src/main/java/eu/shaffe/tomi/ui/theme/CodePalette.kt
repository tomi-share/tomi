package eu.shaffe.tomi.ui.theme

import kotlin.math.pow

/**
 * Colours of highlighted code (markdown code blocks and the code file screen), derived from `design/tomi/tokens.json`.
 * Plain ARGB ints so that it is unit tested without Android.
 *
 * Rules: keywords brand, functions and types plum, strings success green, numbers and constants ochre, comments muted text,
 * operators and punctuation a neutral between text and muted, tags plum, attributes ochre. `color.identity.sun` (#FFC85C) is
 * reserved to the symbol and never used as a text colour. Every pair (token on [background]) reaches the WCAG AA ratio of 4.5:1.
 */
class CodePalette(
    val background: Int,
    val text: Int,
    val keyword: Int,
    val function: Int,
    val type: Int,
    val string: Int,
    val number: Int,
    val constant: Int,
    val comment: Int,
    val operator: Int,
    val punctuation: Int,
    val variable: Int,
    val property: Int,
    val tag: Int,
    val attribute: Int,
    val embedded: Int,
    /** Search highlight: every occurrence. Ochre tint (color.*.secondary), kept readable under [text]. */
    val searchMatch: Int,
    /** Search highlight: the current occurrence, more pressed. */
    val searchCurrent: Int,
    /** Line numbers and the gutter. */
    val gutter: Int,
) {
    /** Colours of the tokens, for the contrast check. */
    val tokenColors: List<Int>
        get() = listOf(text, keyword, function, type, string, number, constant, comment, operator, punctuation, variable, property, tag, attribute, embedded)

    companion object {
        val Light = CodePalette(
            background = 0xFFECE9E2.toInt(),
            text = 0xFF1B2130.toInt(),
            keyword = 0xFF2E4C80.toInt(),
            function = 0xFF6F4F7C.toInt(),
            type = 0xFF6F4F7C.toInt(),
            string = 0xFF2E6B45.toInt(),
            number = 0xFF8C5F14.toInt(),
            constant = 0xFF8C5F14.toInt(),
            comment = 0xFF565C6B.toInt(),
            operator = 0xFF3A4152.toInt(),
            punctuation = 0xFF3A4152.toInt(),
            variable = 0xFF1B2130.toInt(),
            property = 0xFF2E4C80.toInt(),
            tag = 0xFF6F4F7C.toInt(),
            attribute = 0xFF8C5F14.toInt(),
            embedded = 0xFF1B2130.toInt(),
            searchMatch = 0xFFE9CF9B.toInt(),
            searchCurrent = 0xFFD9A441.toInt(),
            gutter = 0xFF565C6B.toInt(),
        )

        val Dark = CodePalette(
            background = 0xFF242A38.toInt(),
            text = 0xFFE9E7E1.toInt(),
            keyword = 0xFFA9BFEA.toInt(),
            function = 0xFFCDB3D9.toInt(),
            type = 0xFFCDB3D9.toInt(),
            string = 0xFF8FCFA6.toInt(),
            number = 0xFFE0B160.toInt(),
            constant = 0xFFE0B160.toInt(),
            comment = 0xFFA6ABB8.toInt(),
            operator = 0xFFC3C1BB.toInt(),
            punctuation = 0xFFC3C1BB.toInt(),
            variable = 0xFFE9E7E1.toInt(),
            property = 0xFFA9BFEA.toInt(),
            tag = 0xFFCDB3D9.toInt(),
            attribute = 0xFFE0B160.toInt(),
            embedded = 0xFFE9E7E1.toInt(),
            searchMatch = 0xFF4A3B1C.toInt(),
            searchCurrent = 0xFF7A5A1A.toInt(),
            gutter = 0xFFA6ABB8.toInt(),
        )

        fun of(dark: Boolean): CodePalette = if (dark) Dark else Light

        /** The identity colour reserved to the symbol: must never appear in [CodePalette]. */
        const val IDENTITY_SUN = 0xFFFFC85C.toInt()

        /** WCAG 2.x contrast ratio between two opaque ARGB colours (1.0 to 21.0). */
        fun contrast(a: Int, b: Int): Double {
            val la = luminance(a)
            val lb = luminance(b)
            return (maxOf(la, lb) + 0.05) / (minOf(la, lb) + 0.05)
        }

        private fun luminance(argb: Int): Double {
            fun channel(shift: Int): Double {
                val c = ((argb shr shift) and 0xFF) / 255.0
                return if (c <= 0.03928) c / 12.92 else ((c + 0.055) / 1.055).pow(2.4)
            }
            return 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
        }
    }
}
