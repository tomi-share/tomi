package eu.shaffe.tomi.ui.render

import com.swmansion.enriched.markdown.MarkdownOptions
import com.swmansion.enriched.markdown.parser.Md4cFlags
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Test

/**
 * The guard and the renderer must parse markdown with the same md4c flags, or the guard could miss an image node that the renderer
 * would fetch (a tracking pixel). The real parser is native and cannot run here: what is checked is the configuration.
 */
class MarkdownEngineConfigTest {
    private fun assertSameFlags(flags: Md4cFlags, options: MarkdownOptions) {
        assertEquals("underline", options.underline, flags.underline)
        assertEquals("superscript", options.superscript, flags.superscript)
        assertEquals("subscript", options.subscript, flags.subscript)
        assertEquals("highlight", options.highlight, flags.highlight)
        assertEquals("permissiveAutolinks", options.permissiveAutolinks, flags.permissiveAutolinks)
        assertEquals("hardSoftBreaks", options.hardSoftBreaks, flags.hardSoftBreaks)
        assertEquals("preserveBlankLines", options.preserveBlankLines, flags.preserveBlankLines)
        assertEquals("admonitions", options.admonitions, flags.admonitions)
    }

    @Test
    fun theGuardParsesWithTheFlagsOfTheOptionsTheRendererUses() {
        assertSame("the renderer is given the options of the shared config", MarkdownOptions.Default, MarkdownEngineConfig.options)
        assertSameFlags(MarkdownEngineConfig.parserFlags, MarkdownEngineConfig.options)
        assertFalse("the renderer never enables LaTeX math", MarkdownEngineConfig.parserFlags.latexMath)
    }

    @Test
    fun everyOptionIsCarriedOverToTheFlags() {
        // Not only the defaults: flip each option, the flags must follow (a copy of the mapping that forgot one would be caught).
        val all = MarkdownOptions(
            underline = true,
            superscript = true,
            subscript = true,
            highlight = true,
            permissiveAutolinks = true,
            hardSoftBreaks = true,
            preserveBlankLines = true,
            admonitions = true,
        )
        assertSameFlags(MarkdownEngineConfig.flagsFor(all), all)
        val none = all.copy(
            underline = false,
            superscript = false,
            subscript = false,
            highlight = false,
            permissiveAutolinks = false,
            hardSoftBreaks = false,
            preserveBlankLines = false,
            admonitions = false,
        )
        assertSameFlags(MarkdownEngineConfig.flagsFor(none), none)
    }

    @Test
    fun theLibraryDefaultsStillAgreeSoAnUpgradeThatChangesOneFailsHere() {
        // The guard used to call Md4cFlags.Default while the renderer used MarkdownOptions.Default: equal today, by luck of the defaults.
        assertSameFlags(Md4cFlags.Default, MarkdownOptions.Default)
        assertEquals(Md4cFlags.Default, MarkdownEngineConfig.parserFlags)
    }
}
