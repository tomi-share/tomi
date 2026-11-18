package eu.shaffe.tomi.share

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SharePayloadParserTest {
    // Stands for PackageManager.resolveContentProvider: Tomi serves exactly one authority here.
    private val own = PackageManagerOwnAuthorityCheck("eu.shaffe.tomi") { authority ->
        if (authority == "eu.shaffe.tomi.fileprovider") "eu.shaffe.tomi" else null
    }

    private fun parse(input: ShareInput) = SharePayloadParser.parse(input, own)

    @Test
    fun chromeShareWithTitleAndUrlInText() {
        val parsed = parse(ShareInput(text = "Kotlin – Wikipédia https://fr.wikipedia.org/wiki/Kotlin"))
        assertEquals("https://fr.wikipedia.org/wiki/Kotlin", parsed.url)
        assertEquals("Kotlin – Wikipédia", parsed.title)
        assertNull(parsed.text)
    }

    @Test
    fun titleAndUrlOnSeparateLines() {
        val parsed = parse(ShareInput(text = "Un article\nhttps://example.org/a?b=1&c=2"))
        assertEquals("https://example.org/a?b=1&c=2", parsed.url)
        assertEquals("Un article", parsed.title)
    }

    @Test
    fun subjectBecomesTitleAndBareUrlIsTheLink() {
        val parsed = parse(ShareInput(subject = "Titre de la page", text = "https://example.org/"))
        assertEquals("https://example.org/", parsed.url)
        assertEquals("Titre de la page", parsed.title)
        assertNull(parsed.text)
    }

    @Test
    fun subjectEqualToLeftoverIsNotDuplicated() {
        val parsed = parse(ShareInput(subject = "Titre", text = "Titre https://example.org/x"))
        assertEquals("Titre", parsed.title)
        assertNull(parsed.text)
        assertEquals("https://example.org/x", parsed.url)
    }

    @Test
    fun subjectAndDifferentLeftoverKeepsBothAsComposite() {
        val parsed = parse(ShareInput(subject = "Sujet", text = "Voir cette page https://example.org/x"))
        assertEquals("Sujet", parsed.title)
        assertEquals("Voir cette page", parsed.text?.plain)
        assertEquals("https://example.org/x", parsed.url)
    }

    @Test
    fun trailingPunctuationIsNotPartOfTheUrl() {
        val parsed = parse(ShareInput(text = "Regarde https://example.org/page."))
        assertEquals("https://example.org/page", parsed.url)
    }

    @Test
    fun balancedParenthesisStaysInTheUrl() {
        val parsed = parse(ShareInput(text = "https://en.wikipedia.org/wiki/Kotlin_(programming_language)"))
        assertEquals("https://en.wikipedia.org/wiki/Kotlin_(programming_language)", parsed.url)
    }

    @Test
    fun urlInTheMiddleOfASentenceStaysText() {
        val parsed = parse(ShareInput(text = "See https://example.org/x for details"))
        assertNull(parsed.url)
        assertEquals("See https://example.org/x for details", parsed.text?.plain)
    }

    @Test
    fun twoUrlsStayText() {
        val parsed = parse(ShareInput(text = "https://a.example/1 https://b.example/2"))
        assertNull(parsed.url)
        assertEquals("https://a.example/1 https://b.example/2", parsed.text?.plain)
    }

    @Test
    fun nonHttpSchemesAreNotLinks() {
        val parsed = parse(ShareInput(text = "javascript:alert(1)"))
        assertNull(parsed.url)
        val file = parse(ShareInput(text = "file:///data/data/eu.shaffe.tomi/x"))
        assertNull(file.url)
    }

    @Test
    fun plainTextWithSubject() {
        val parsed = parse(ShareInput(subject = "Courses", text = "Lait\nPain"))
        assertEquals("Courses", parsed.title)
        assertEquals("Lait\nPain", parsed.text?.plain)
        assertNull(parsed.url)
    }

    @Test
    fun htmlTextIsKeptNextToPlain() {
        val parsed = parse(ShareInput(text = "Bonjour monde", htmlText = "<p>Bonjour <b>monde</b></p>"))
        assertEquals("Bonjour monde", parsed.text?.plain)
        assertEquals("<p>Bonjour <b>monde</b></p>", parsed.text?.html)
    }

    @Test
    fun htmlOnlyGetsAPlainFallback() {
        val parsed = parse(ShareInput(htmlText = "<h1>Titre</h1><p>Un &amp; deux</p><script>x()</script>"))
        assertEquals("Titre\nUn & deux", parsed.text?.plain)
        assertTrue(parsed.text?.html != null)
    }

    @Test
    fun markdownSyntaxIsDetectedAsSourceFormat() {
        val parsed = parse(ShareInput(text = "# Titre\n\n- un\n- deux"))
        assertEquals("# Titre\n\n- un\n- deux", parsed.text?.markdown)
    }

    @Test
    fun plainSentenceIsNotMarkdown() {
        val parsed = parse(ShareInput(text = "Rendez-vous demain à 14 h."))
        assertNull(parsed.text?.markdown)
    }

    @Test
    fun streamsFromOtherAppsAreKeptButOwnAndFileUrisAreDropped() {
        val parsed = parse(
            ShareInput(
                streams = listOf(
                    SharedStream("content://media/external/images/1", "image/png", "a.png", 10),
                    SharedStream("content://eu.shaffe.tomi.fileprovider/x", null, null, null),
                    SharedStream("file:///data/data/eu.shaffe.tomi/databases/x", null, null, null),
                ),
            ),
        )
        assertEquals(listOf("content://media/external/images/1"), parsed.streams.map { it.uri })
    }

    @Test
    fun anIntentWithOneTextAndTwoFilesIsASingleCompositeItem() {
        val parsed = parse(
            ShareInput(
                subject = "Photos",
                text = "Voici",
                streams = listOf(
                    SharedStream("content://media/1", "image/jpeg", "1.jpg", 1),
                    SharedStream("content://media/2", "image/jpeg", "2.jpg", 2),
                ),
            ),
        )
        assertEquals("Photos", parsed.title)
        assertEquals("Voici", parsed.text?.plain)
        assertEquals(2, parsed.streams.size)
    }

    @Test
    fun emptyShareIsEmpty() {
        assertTrue(parse(ShareInput(text = "   ", subject = "")).isEmpty)
    }

    @Test
    fun controlAndBidiCharactersAreRemoved() {
        val parsed = parse(ShareInput(subject = "A\u202EB\u0007C", text = "x"))
        assertEquals("ABC", parsed.title)
    }

    @Test
    fun sensitiveClipOrPasswordManagerMarksTheShareAsSecret() {
        assertTrue(parse(ShareInput(text = "hello world", markedSensitive = true)).looksSecret)
        assertTrue(parse(ShareInput(text = "hello world", sourcePackage = "com.dashlane")).looksSecret)
        assertFalse(parse(ShareInput(text = "hello world", sourcePackage = "com.android.chrome")).looksSecret)
    }

    @Test
    fun aPasswordLikeTokenLooksSecretButALinkNeverDoes() {
        assertTrue(parse(ShareInput(text = "Xk9#mPq2!vLw")).looksSecret)
        assertFalse(parse(ShareInput(text = "https://example.org/Xk9mPq2vLw")).looksSecret)
    }
}
