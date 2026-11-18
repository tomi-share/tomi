package eu.shaffe.tomi.ui.render

import com.swmansion.enriched.markdown.parser.MarkdownASTNode
import com.swmansion.enriched.markdown.parser.MarkdownASTNode.NodeType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The markdown renderer must never fetch a remote image (offline requirement). The guard escapes image openers and refuses any
 * markdown whose parsed tree still has an image.
 */
class MarkdownGuardTest {
    private fun node(type: NodeType, vararg children: MarkdownASTNode) = MarkdownASTNode(type = type, children = children.toList())

    private val noImage = node(NodeType.Document, node(NodeType.Paragraph, node(NodeType.Text), node(NodeType.Link)))

    // --- neutralizeImages -------------------------------------------------------------------------------------------

    @Test
    fun anImageOpenerIsEscapedSoItBecomesALink() {
        assertEquals("\\![alt](https://t.example/pixel.png)", MarkdownGuard.neutralizeImages("![alt](https://t.example/pixel.png)"))
        assertEquals("a \\![x](u) b \\![y](v)", MarkdownGuard.neutralizeImages("a ![x](u) b ![y](v)"))
    }

    @Test
    fun referenceAndCollapsedImagesAreEscapedToo() {
        assertEquals("\\![alt][ref]\n\n[ref]: https://t.example/p.png", MarkdownGuard.neutralizeImages("![alt][ref]\n\n[ref]: https://t.example/p.png"))
        assertEquals("\\![ref][]", MarkdownGuard.neutralizeImages("![ref][]"))
        assertEquals("\\![ref]", MarkdownGuard.neutralizeImages("![ref]"))
    }

    @Test
    fun anImageInsideALinkIsEscaped() {
        assertEquals("[\\![a](u)](v)", MarkdownGuard.neutralizeImages("[![a](u)](v)"))
    }

    @Test
    fun textWithoutImageIsReturnedAsIs() {
        val text = "# Titre\n\n- a\n- b\n\n**gras** et [lien](https://a.example) ! [pas une image]\n"
        assertSame(text, MarkdownGuard.neutralizeImages(text))
    }

    @Test
    fun anAlreadyEscapedOpenerIsNotEscapedTwice() {
        assertEquals("\\![a](u)", MarkdownGuard.neutralizeImages("\\![a](u)"))
    }

    @Test
    fun aBackslashBeforeTheOpenerDoesNotHideTheImage() {
        // "\\" is a literal backslash, so the "![" after it still opens an image.
        assertEquals("\\\\\\![a](u)", MarkdownGuard.neutralizeImages("\\\\![a](u)"))
    }

    @Test
    fun codeSpansAndFencedBlocksKeepTheirCharactersLiteral() {
        assertEquals("`![a](u)` et \\![b](v)", MarkdownGuard.neutralizeImages("`![a](u)` et ![b](v)"))
        assertEquals("``a ` ![x](u)`` \\![y](v)", MarkdownGuard.neutralizeImages("``a ` ![x](u)`` ![y](v)"))
        val fenced = "```md\n![a](u)\n```\n![b](v)"
        assertEquals("```md\n![a](u)\n```\n\\![b](v)", MarkdownGuard.neutralizeImages(fenced))
        val tilde = "~~~\n![a](u)\n~~~~\n![b](v)"
        assertEquals("~~~\n![a](u)\n~~~~\n\\![b](v)", MarkdownGuard.neutralizeImages(tilde))
    }

    @Test
    fun anUnclosedFenceKeepsEverythingAfterItLiteralLikeMarkdownDoes() {
        assertEquals("```\n![a](u)\n![b](v)", MarkdownGuard.neutralizeImages("```\n![a](u)\n![b](v)"))
    }

    @Test
    fun aFenceNeedsThreeCharactersAndAShortOneDoesNotCloseTheBlock() {
        assertEquals("\\![a](u) ``", MarkdownGuard.neutralizeImages("![a](u) ``"))
        assertEquals("````\n```\n![a](u)\n````\n\\![b](v)", MarkdownGuard.neutralizeImages("````\n```\n![a](u)\n````\n![b](v)"))
    }

    @Test
    fun anUnclosedBacktickIsPlainTextAndTheImageAfterItIsEscaped() {
        assertEquals("` \\![a](u)", MarkdownGuard.neutralizeImages("` ![a](u)"))
    }

    @Test
    fun lineEndingsAndTrailingNewlinesArePreserved() {
        assertEquals("a\r\n\\![b](u)\r\n", MarkdownGuard.neutralizeImages("a\r\n![b](u)\r\n"))
        assertEquals("\\![b](u)\n\n", MarkdownGuard.neutralizeImages("![b](u)\n\n"))
    }

    @Test
    fun anIndentedLineIsEscapedToo() {
        // Could be a lazy continuation or a list item: escaping is the safe side (the tree check is the backstop).
        assertEquals("    \\![a](u)", MarkdownGuard.neutralizeImages("    ![a](u)"))
    }

    // --- containsImage / safeMarkdown ----------------------------------------------------------------------------------

    @Test
    fun anImageAnywhereInTheTreeIsFound() {
        assertFalse(MarkdownGuard.containsImage(noImage))
        val deep = node(NodeType.Document, node(NodeType.Blockquote, node(NodeType.UnorderedList, node(NodeType.ListItem, node(NodeType.Image)))))
        assertTrue(MarkdownGuard.containsImage(deep))
        val inTable = node(NodeType.Document, node(NodeType.Table, node(NodeType.TableBody, node(NodeType.TableRow, node(NodeType.TableCell, node(NodeType.Image))))))
        assertTrue(MarkdownGuard.containsImage(inTable))
        assertTrue(MarkdownGuard.containsImage(node(NodeType.Image)))
    }

    @Test
    fun markdownWithoutImageIsHandedToTheRendererEscapedAndChecked() {
        var parsed: String? = null
        val result = MarkdownGuard.safeMarkdown("a ![x](https://t.example/p.png)") { parsed = it; noImage }
        assertEquals("a \\![x](https://t.example/p.png)", result)
        // The tree that is checked is the one of the text the renderer will receive.
        assertEquals(result, parsed)
    }

    @Test
    fun markdownWhoseTreeStillHasAnImageIsRefused() {
        val withImage = node(NodeType.Document, node(NodeType.Paragraph, node(NodeType.Image)))
        assertNull(MarkdownGuard.safeMarkdown("anything") { withImage })
    }

    @Test
    fun markdownThatCannotBeParsedIsRefused() {
        assertNull(MarkdownGuard.safeMarkdown("# a") { null })
        assertNull(MarkdownGuard.safeMarkdown("# a") { throw IllegalStateException("native parser failed") })
    }

    @Test
    fun blankMarkdownNeedsNoParsing() {
        assertEquals("  \n", MarkdownGuard.safeMarkdown("  \n") { error("must not be called") })
        assertEquals("", MarkdownGuard.safeMarkdown("") { error("must not be called") })
    }

    @Test
    fun hugeMarkdownIsNotRendered() {
        val huge = "a".repeat(MarkdownGuard.MAX_CHARS + 1)
        assertNull(MarkdownGuard.safeMarkdown(huge) { error("must not be called") })
        assertEquals("a".repeat(MarkdownGuard.MAX_CHARS), MarkdownGuard.safeMarkdown("a".repeat(MarkdownGuard.MAX_CHARS)) { noImage })
    }
}
