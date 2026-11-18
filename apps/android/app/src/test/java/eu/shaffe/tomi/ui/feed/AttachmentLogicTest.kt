package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Attachment
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AttachmentLogicTest {
    private val thumb = byteArrayOf(1, 2, 3)
    private fun image(name: String = "a.jpg", thumbnail: ByteArray? = thumb) = Attachment(name, "image/jpeg", 1000, thumbnail)
    private fun file(name: String) = Attachment(name, "application/pdf", 1000)

    // ---------------------------------------------------------------- extension

    @Test
    fun splitsTheLastExtension() {
        val parts = AttachmentLogic.splitExtension("Courrier banque.pdf")
        assertEquals("Courrier banque", parts.radical)
        assertEquals(".pdf", parts.extension)
        assertEquals(".gz", AttachmentLogic.splitExtension("archive.tar.gz").extension)
    }

    @Test
    fun noExtensionWhenThereIsNone() {
        for (name in listOf("LISEZMOI", ".gitignore", "fin.", "note. finale", "a." + "x".repeat(11))) {
            val parts = AttachmentLogic.splitExtension(name)
            assertEquals(name, parts.radical)
            assertEquals("", parts.extension)
        }
    }

    @Test
    fun badgeShowsTheExtensionInCapitalsUpToFourLetters() {
        assertEquals("PDF", AttachmentLogic.badgeExtension("devis.pdf"))
        assertEquals("KT", AttachmentLogic.badgeExtension("FeedViewModel.kt"))
        assertEquals("WEBP", AttachmentLogic.badgeExtension("photo.webp"))
        assertEquals("GZ", AttachmentLogic.badgeExtension("a.tar.gz"))
    }

    @Test
    fun badgeFallsBackToAnIconWithoutUsableExtension() {
        assertNull(AttachmentLogic.badgeExtension("schema.drawio"))
        assertNull(AttachmentLogic.badgeExtension("LISEZMOI"))
        assertNull(AttachmentLogic.badgeExtension(".gitignore"))
        assertNull(AttachmentLogic.badgeExtension("a.p-f"))
    }

    // ---------------------------------------------------------------- cutting before the extension

    /** A text "fits" when it has at most [width] characters (one line of that width). */
    private fun fit(name: String, width: Int) = AttachmentLogic.fitFileName(name) { it.length <= width }

    @Test
    fun aNameThatFitsIsUntouched() {
        assertEquals("devis.pdf", fit("devis.pdf", 20))
        assertEquals("devis.pdf", fit("devis.pdf", 9))
    }

    @Test
    fun theExtensionIsKeptAndTheRadicalIsCutAsLateAsPossible() {
        val name = "Courrier banque fonds de garantie locative résiliation du bail 2026.pdf"
        val cut = fit(name, 31)
        assertEquals("Courrier banque fonds de g….pdf", cut)
        assertEquals(31, cut.length)
        assertTrue(name.startsWith(cut.removeSuffix("….pdf")))
    }

    @Test
    fun theCutRadicalLosesItsTrailingSpace() {
        // "Courrier banque" + "…" would otherwise read "Courrier banque …".
        val cut = fit("Courrier banque fonds.pdf", 21)
        assertEquals("Courrier banque….pdf", cut)
    }

    @Test
    fun aNameWithoutExtensionIsCutAtTheEnd() {
        assertEquals("abcdefghi…", fit("abcdefghijklmnopqrstuvwxyz", 10))
    }

    @Test
    fun whenNothingFitsTheFloorIsTheEllipsisAndTheExtension() {
        assertEquals("….pdf", fit("a-very-long-name.pdf", 2))
    }

    @Test
    fun neverCutsInTheMiddleOfASurrogatePair() {
        // 3 characters of radical would end in the middle of the emoji (2 UTF-16 units): 2 are kept instead.
        val cut = AttachmentLogic.fitFileName("ab😀cdef.png") { it.length <= 8 }
        assertEquals("ab….png", cut)
        assertTrue(cut.none { Character.isSurrogate(it) })
    }

    @Test
    fun theSearchIsLogarithmic() {
        var calls = 0
        AttachmentLogic.fitFileName("x".repeat(4000) + ".pdf") {
            calls++
            it.length <= 2000
        }
        assertTrue("$calls measures", calls <= 20)
    }

    // ---------------------------------------------------------------- plan

    private fun items(plan: AttachmentLogic.Plan) = plan as AttachmentLogic.Plan.Items
    private fun mosaic(plan: AttachmentLogic.Plan) = plan as AttachmentLogic.Plan.Mosaic

    @Test
    fun aSingleImageIsAFrameInTheFeedAndTheDetail() {
        for (expanded in listOf(false, true)) {
            val plan = items(AttachmentLogic.plan(listOf(image()), expanded, showThumbnails = true))
            assertEquals(listOf(AttachmentLogic.Look.Frame), plan.entries.map { it.look })
            assertEquals(0, plan.more)
        }
    }

    @Test
    fun aSingleImageWithoutThumbnailIsATile() {
        val plan = items(AttachmentLogic.plan(listOf(image(thumbnail = null)), expanded = false, showThumbnails = true))
        assertEquals(listOf(AttachmentLogic.Look.Tile), plan.entries.map { it.look })
    }

    @Test
    fun imagesOnlyMakeAMosaicOfThreeCellsAndCountTheRest() {
        val plan = mosaic(AttachmentLogic.plan(List(5) { image() }, expanded = false, showThumbnails = true))
        assertEquals(3, plan.shown)
        assertEquals(2, plan.more)
    }

    @Test
    fun twoOrThreeImagesHaveNoPlusCell() {
        val two = mosaic(AttachmentLogic.plan(List(2) { image() }, expanded = false, showThumbnails = true))
        assertEquals(2, two.shown)
        assertEquals(0, two.more)
        val three = mosaic(AttachmentLogic.plan(List(3) { image() }, expanded = false, showThumbnails = true))
        assertEquals(3, three.shown)
        assertEquals(0, three.more)
    }

    @Test
    fun fourImagesGiveAPlusOneCell() {
        val plan = mosaic(AttachmentLogic.plan(List(4) { image() }, expanded = false, showThumbnails = true))
        assertEquals(3, plan.shown)
        assertEquals(1, plan.more)
    }

    @Test
    fun aFourthImageWithoutThumbnailKeepsTheMosaic() {
        val batch = List(3) { image() } + image(thumbnail = null)
        val plan = mosaic(AttachmentLogic.plan(batch, expanded = false, showThumbnails = true))
        assertEquals(1, plan.more)
    }

    @Test
    fun noAttachmentIsNoItem() {
        val plan = items(AttachmentLogic.plan(emptyList(), expanded = false, showThumbnails = true))
        assertTrue(plan.entries.isEmpty())
        assertEquals(0, plan.more)
    }

    @Test
    fun anEmptyNameIsUntouchedAndADotFileKeepsItsLastExtension() {
        assertEquals("", fit("", 5))
        assertEquals(".local", AttachmentLogic.splitExtension(".env.local").extension)
        assertEquals(".env", AttachmentLogic.splitExtension(".env.local").radical)
    }

    @Test
    fun aMixedBatchIsThreeTilesThenTheRest() {
        val batch = listOf(image(), file("a.pdf"), file("b.txt"), file("c.txt"), file("d.txt"))
        val plan = items(AttachmentLogic.plan(batch, expanded = false, showThumbnails = true))
        // In a list, the image is a tile with a mini-thumbnail (the frame is for a lone image).
        assertEquals(List(3) { AttachmentLogic.Look.Tile }, plan.entries.map { it.look })
        assertEquals(listOf(0, 1, 2), plan.entries.map { it.index })
        assertEquals(2, plan.more)
    }

    @Test
    fun imagesWithAMissingThumbnailFallBackToTiles() {
        val plan = AttachmentLogic.plan(listOf(image(), image(thumbnail = null)), expanded = false, showThumbnails = true)
        assertEquals(2, items(plan).entries.size)
    }

    @Test
    fun aHiddenItemNeverShowsThumbnailsNorAMosaic() {
        val plan = items(AttachmentLogic.plan(List(4) { image() }, expanded = false, showThumbnails = false))
        assertTrue(plan.entries.all { it.look == AttachmentLogic.Look.Tile })
        assertEquals(1, plan.more)
        val single = items(AttachmentLogic.plan(listOf(image()), expanded = true, showThumbnails = false))
        assertEquals(AttachmentLogic.Look.Tile, single.entries.single().look)
    }

    @Test
    fun theDetailListsEverythingInOrder() {
        val all = listOf(image("1.jpg"), file("2.pdf"), image("3.jpg"), file("4.txt"), file("5.txt"))
        val plan = items(AttachmentLogic.plan(all, expanded = true, showThumbnails = true))
        assertEquals(listOf(0, 1, 2, 3, 4), plan.entries.map { it.index })
        assertEquals(0, plan.more)
        val frame = AttachmentLogic.Look.Frame
        val tile = AttachmentLogic.Look.Tile
        assertEquals(listOf(frame, tile, frame, tile, tile), plan.entries.map { it.look })
    }

    @Test
    fun theDetailOfImagesOnlyIsNotAMosaic() {
        val plan = AttachmentLogic.plan(List(5) { image() }, expanded = true, showThumbnails = true)
        assertEquals(5, items(plan).entries.size)
    }

    // ---------------------------------------------------------------- summary

    @Test
    fun summaryFromTwoAttachmentsOn() {
        assertFalse(AttachmentLogic.showsSummary(listOf(file("a.pdf"))))
        assertTrue(AttachmentLogic.showsSummary(listOf(file("a.pdf"), file("b.pdf"))))
        assertEquals(2000L, AttachmentLogic.totalSize(listOf(file("a.pdf"), file("b.pdf"))))
        assertTrue(AttachmentLogic.isAllImages(listOf(image(), image())))
        assertFalse(AttachmentLogic.isAllImages(listOf(image(), file("a.pdf"))))
        assertFalse(AttachmentLogic.isAllImages(emptyList()))
    }
}
