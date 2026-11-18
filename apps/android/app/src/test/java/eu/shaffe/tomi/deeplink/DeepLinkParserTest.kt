package eu.shaffe.tomi.deeplink

import eu.shaffe.tomi.core.TestLinkReader
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DeepLinkParserTest {
    private fun parse(raw: String?) = DeepLinkParser.parse(raw, TestLinkReader)


    @Test
    fun sendWithUrlAndTitle() {
        val link = parse("tomi-share://send?url=https%3A%2F%2Fexample.org%2Fa%3Fb%3D1&title=Mon%20titre") as DeepLink.Send
        assertEquals("https://example.org/a?b=1", link.url)
        assertEquals("Mon titre", link.title)
        assertNull(link.text)
        assertFalse(link.droppedParameters)
    }

    @Test
    fun sendWithText() {
        val link = parse("tomi-share://send?text=Bonjour%20%C3%A0%20tous") as DeepLink.Send
        assertEquals("Bonjour à tous", link.text)
    }

    @Test
    fun nonHttpUrlIsDropped() {
        val link = parse("tomi-share://send?url=javascript%3Aalert(1)") as DeepLink.Send
        assertNull(link.url)
        assertTrue(link.droppedParameters)
    }

    @Test
    fun oversizedParametersAreDropped() {
        val long = "a".repeat(DeepLinkParser.MAX_TEXT_PARAM + 1)
        val link = parse("tomi-share://send?text=$long") as DeepLink.Send
        assertNull(link.text)
        assertTrue(link.droppedParameters)
    }

    @Test
    fun oversizedUriIsRejectedEntirely() {
        assertNull(parse("tomi-share://send?text=" + "a".repeat(DeepLinkParser.MAX_URI_LENGTH)))
    }

    @Test
    fun thereIsNoSecretParameter() {
        val link = parse("tomi-share://send?text=abc&secret=true") as DeepLink.Send
        assertEquals("abc", link.text)
        // The Send command has no field that could carry a secret flag or trigger a send.
        assertEquals(setOf("title", "url", "text", "droppedParameters"), DeepLink.Send::class.java.declaredFields.map { it.name }.filterNot { it.startsWith("$") }.toSet())
    }

    @Test
    fun itemLinkValidatesTheId() {
        assertEquals(DeepLink.Item("abc-123_X"), parse("tomi-share://item/abc-123_X"))
        assertNull(parse("tomi-share://item/"))
        assertNull(parse("tomi-share://item/../../etc"))
        assertNull(parse("tomi-share://item/" + "a".repeat(65)))
    }

    @Test
    fun openLink() {
        assertEquals(DeepLink.Open, parse("tomi-share://open"))
    }

    @Test
    fun unknownCommandsAreIgnored() {
        assertNull(parse("tomi-share://delete?id=1"))
        assertNull(parse("tomi-share://revoke/dev-1"))
        assertNull(parse("tomi-share://settings?relay=https://evil.example"))
        assertNull(parse("ftp://example.org"))
        assertNull(parse(null))
        assertNull(parse("not a uri"))
    }

    @Test
    fun customSchemePairingLinkNamesNoRelay() {
        assertEquals(
            DeepLink.Pair("tomi-share://pair#4821-crayon-renard", relayUrl = null),
            parse("tomi-share://pair#4821-crayon-renard"),
        )
        // The code is in the fragment: a query string (it reaches logs) or no fragment at all is not a pairing link.
        assertNull(parse("tomi-share://pair"))
        assertNull(parse("tomi-share://pair?code=4821-crayon-renard"))
        assertNull(parse("tomi-share://pair/4821-crayon-renard"))
        assertNull(parse("tomi-share://pair#" + "a".repeat(600)))
        // Another host cannot pose as the pairing link.
        assertNull(parse("tomi-share://evil.example#4821-crayon-renard"))
    }

    @Test
    fun aPairingLinkNeverPrintsItsCodeOrItsToken() {
        assertEquals("Pair(<redacted>)", parse("tomi-share://pair#4821-crayon-renard").toString())
        assertEquals("Pair(<redacted>)", parse("https://relay.example.org/l/pair#4821-crayon-renard").toString())
        assertEquals("Setup(<redacted>)", parse("https://relay.example.org/create#$TOKEN").toString())
    }

    @Test
    fun anAppLinkOfAnyRelayCarriesItsRelayAndIsRebuiltFromWhatTheCoreRead() {
        val ok = parse("https://relay.example.org/l/pair#4821-crayon-renard") as DeepLink.Pair
        assertEquals("https://relay.example.org/l/pair#4821-crayon-renard", ok.payload)
        assertEquals("https://relay.example.org", ok.relayUrl)

        val other = parse("https://Mon-Relais.example.net:8443/l/pair#4821-crayon-renard") as DeepLink.Pair
        assertEquals("https://mon-relais.example.net:8443/l/pair#4821-crayon-renard", other.payload)
        assertEquals("https://mon-relais.example.net:8443", other.relayUrl)

        // Nothing but the relay and the code survives: no query, no user information.
        assertNull(parse("https://user@relay.example.org/l/pair#4821-crayon-renard"))
        assertNull(parse("https://relay.example.org/l/pair"))
        assertNull(parse("https://relay.example.org/l/pair#" + "a".repeat(600)))
        assertNull(parse("http://relay.example.org/l/pair#4821-crayon-renard"))
        assertNull(parse("https://relay.example.org/l/pair?code=4821-crayon-renard"))
    }

    @Test
    fun aCreationLinkOpensTheCreationScreenWithTheRelayAndTheToken() {
        val fragment = parse("https://relay.example.org/create#$TOKEN") as DeepLink.Setup
        assertEquals("https://relay.example.org", fragment.relayUrl)
        assertEquals(TOKEN, fragment.token.reveal())

        val path = parse("https://relay.example.org:8443/create/$TOKEN") as DeepLink.Setup
        assertEquals("https://relay.example.org:8443", path.relayUrl)

        assertNull(parse("https://relay.example.org/create"))
        assertNull(parse("https://relay.example.org/create#not-a-token"))
        assertNull(parse("tomi-share://create#$TOKEN"))
    }

    @Test
    fun otherHttpsLinksAreNotForTheApp() {
        assertNull(parse("https://relay.example.org/l/anything"))
        assertNull(parse("https://relay.example.org/other"))
        assertNull(parse("https://example.org/"))
    }

    private companion object {
        const val TOKEN = "y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47"
    }
}
