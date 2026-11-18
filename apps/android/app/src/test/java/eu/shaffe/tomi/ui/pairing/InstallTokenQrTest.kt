package eu.shaffe.tomi.ui.pairing

import eu.shaffe.tomi.core.TestLinkReader
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What a QR code scanned on "Create your space" may be: a creation link of a relay (relay + token), or the bare installation token (the
 * relay is then the one already known or typed). Never a pairing link or any other QR code.
 */
class InstallTokenQrTest {
    // 32 characters of the relay's base32 alphabet, in 8 groups of 4 (apps/relay/src/install.rs).
    private val token = "y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47"

    private fun parse(text: String) = InstallTokenQr.parse(text, TestLinkReader)

    private fun bare(text: String): String? = (parse(text) as? ScannedInstall.Token)?.token?.reveal()

    @Test
    fun aTokenIsAcceptedAsItIsScanned() {
        assertEquals(token, bare(token))
    }

    @Test
    fun surroundingBlanksAreDroppedAndTheCaseDoesNotMatter() {
        assertEquals(token, bare("  $token \n"))
        assertEquals(token.uppercase(), bare(token.uppercase()))
    }

    @Test
    fun aTokenWithoutDashesIsAccepted() {
        val noDashes = token.replace("-", "")
        assertEquals(noDashes, bare(noDashes))
    }

    @Test
    fun aCreationLinkGivesTheRelayAndTheToken() {
        val fragment = parse("https://relay.example.org/create#$token") as ScannedInstall.Link
        assertEquals("https://relay.example.org", fragment.relayUrl)
        assertEquals(token, fragment.token.reveal())

        val path = parse("  https://Relay.Example.org:8443/create/$token\n") as ScannedInstall.Link
        assertEquals("https://relay.example.org:8443", path.relayUrl)
        assertEquals(token, path.token.reveal())
    }

    @Test
    fun theScannedContentIsNeverPrinted() {
        assertTrue(!parse("https://relay.example.org/create#$token").toString().contains("y8d6"))
        assertTrue(!parse(token).toString().contains("y8d6"))
    }

    @Test
    fun aPairingLinkIsNotAToken() {
        assertNull(parse("tomi-share://pair#4821-crayon-renard"))
        assertNull(parse("https://relay.example.org/l/pair#4821-crayon-renard"))
        assertNull(parse("4821-crayon-renard"))
    }

    @Test
    fun aCreationLinkNeedsAWellFormedTokenAndAnHttpsRelay() {
        assertNull(parse("https://relay.example.org/create"))
        assertNull(parse("https://relay.example.org/create#y8d6-69p1"))
        assertNull(parse("http://relay.example.org/create#$token"))
        assertNull(parse("https://relay.example.org/other#$token"))
        assertNull(parse("https://user@relay.example.org/create#$token"))
    }

    @Test
    fun anythingElseIsRefused() {
        assertNull(parse(""))
        assertNull(parse("   "))
        assertNull(parse("https://example.org/"))
        // Too short, too long, and letters that are not in the alphabet (i, l, o, u).
        assertNull(parse("y8d6-69p1"))
        assertNull(parse("$token-y8d6-69p1-7b5p"))
        assertNull(parse("i8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47"))
        assertNull(parse("y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5nou"))
        assertNull(parse("x".repeat(65)))
    }
}
