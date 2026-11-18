package eu.shaffe.tomi.ui.pairing

import com.google.zxing.BinaryBitmap
import com.google.zxing.LuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import eu.shaffe.tomi.core.PairingFailure
import eu.shaffe.tomi.core.TestLinkReader
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The pure parts of the pairing screens: what is typed or scanned, the countdown, the QR code, the messages. */
class PairingUiLogicTest {
    // --- What is typed or scanned -----------------------------------------------------------------------------------------

    @Test
    fun aTypedCodeIsReadByTheCoreSoOnlyWhatIsClearlyNotACodeIsRefusedHere() {
        assertTrue(PairingInput.isValidCode("4821-crayon-renard"))
        // The core accepts these (spec/pairing-v1.md 4.4): capitals, separators, accents, a prefix of four letters.
        assertTrue(PairingInput.isValidCode("4821 Crayon RENARD"))
        assertTrue(PairingInput.isValidCode("4821_crayon.renard"))
        assertTrue(PairingInput.isValidCode("4821-crâyon-rena"))
        assertTrue(PairingInput.isValidCode("  4821-crayon-renard  "))

        assertFalse(PairingInput.isValidCode(""))
        assertFalse(PairingInput.isValidCode("   "))
        assertFalse(PairingInput.isValidCode("a".repeat(PairingInput.MAX_CODE_INPUT + 1)))
        assertFalse(PairingInput.isValidCode("4821-crayon\u0000-renard"))
    }

    @Test
    fun aScannedQrCodeMustBeAPairingLinkAndCarriesItsRelay() {
        val ok = PairingInput.linkFromQr("  https://relay.example.org/l/pair#4821-crayon-renard\n", TestLinkReader)!!
        assertEquals("https://relay.example.org/l/pair#4821-crayon-renard", ok.payload)
        assertEquals("https://relay.example.org", ok.relayUrl)

        // Any relay is read, and shown in full later: it is not trusted, not refused.
        val other = PairingInput.linkFromQr("https://autre.example.net/l/pair#4821-crayon-renard", TestLinkReader)!!
        assertEquals("https://autre.example.net", other.relayUrl)

        // The fallback form names no relay: the one of the device is used.
        val fallback = PairingInput.linkFromQr("tomi-share://pair#4821-crayon-renard", TestLinkReader)!!
        assertEquals("tomi-share://pair#4821-crayon-renard", fallback.payload)
        assertNull(fallback.relayUrl)

        assertNull(PairingInput.linkFromQr("http://relay.example.org/l/pair#4821-crayon-renard", TestLinkReader))
        assertNull(PairingInput.linkFromQr("https://relay.example.org/l/pair?code=4821-crayon-renard", TestLinkReader))
        assertNull(PairingInput.linkFromQr("https://relay.example.org/create#y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47", TestLinkReader))
        assertNull(PairingInput.linkFromQr("WIFI:S:home;T:WPA;P:secret;;", TestLinkReader))
        assertNull(PairingInput.linkFromQr("4821-crayon-renard", TestLinkReader))
    }

    // --- Countdown ------------------------------------------------------------------------------------------------------------

    @Test
    fun theCountdownReadsMinutesAndSecondsRoundedUp() {
        assertEquals("2:00", Countdown.format(120_000))
        assertEquals("1:48", Countdown.format(108_000))
        assertEquals("1:48", Countdown.format(107_001))
        assertEquals("0:01", Countdown.format(1))
        assertEquals("0:00", Countdown.format(0))
        assertEquals("0:00", Countdown.format(-5))
        assertEquals("5:00", Countdown.format(300_000))
    }

    // --- QR code ------------------------------------------------------------------------------------------------------------

    /** The modules of [matrix] as a grey image, [scale] pixels per module, [quiet] white modules all around. */
    private class MatrixSource(matrix: QrMatrix, scale: Int, quiet: Int) :
        LuminanceSource((matrix.size + 2 * quiet) * scale, (matrix.size + 2 * quiet) * scale) {
        private val pixels = ByteArray(width * height) { 0xFF.toByte() }.also { image ->
            for (y in 0 until matrix.size) {
                for (x in 0 until matrix.size) {
                    if (!matrix[x, y]) continue
                    for (dy in 0 until scale) {
                        for (dx in 0 until scale) {
                            image[((y + quiet) * scale + dy) * width + (x + quiet) * scale + dx] = 0
                        }
                    }
                }
            }
        }

        override fun getRow(y: Int, row: ByteArray?): ByteArray {
            val out = row ?: ByteArray(width)
            System.arraycopy(pixels, y * width, out, 0, width)
            return out
        }

        override fun getMatrix(): ByteArray = pixels
    }

    @Test
    fun theQrCodeCarriesExactlyTheLinkOfTheCore() {
        val link = "https://relay.example.org/l/pair#4821-crayon-renard"
        val matrix = QrCode.encode(link)
        assertTrue("a square of at least a version 1 code", matrix.size >= 21)

        val decoded = QRCodeReader().decode(BinaryBitmap(HybridBinarizer(MatrixSource(matrix, scale = 4, quiet = QrCode.QUIET_ZONE))))

        assertEquals(link, decoded.text)
    }

    @Test
    fun twoDifferentCodesGiveTwoDifferentQrCodes() {
        fun bits(m: QrMatrix) = (0 until m.size).flatMap { y -> (0 until m.size).map { x -> m[x, y] } }

        val a = QrCode.encode("https://relay.example.org/l/pair#4821-crayon-renard")
        val b = QrCode.encode("https://relay.example.org/l/pair#4822-crayon-renard")
        assertNotEquals(bits(a), bits(b))
    }

    // --- Messages -----------------------------------------------------------------------------------------------------------

    @Test
    fun everyFailureHasAMessageOnEachScreen() {
        for (reason in PairingFailure.entries) {
            assertNotEquals(reason.name, 0, joinFailureMessage(reason))
            assertNotEquals(reason.name, 0, inviteFailureMessage(reason))
            assertNotEquals(reason.name, 0, createFailureMessage(reason))
        }
    }

    @Test
    fun theRelayFailuresAreToldInTheirOwnWords() {
        assertNotEquals(joinFailureMessage(PairingFailure.RelayNotConfigured), joinFailureMessage(PairingFailure.InvalidRelay))
        assertNotEquals(joinFailureMessage(PairingFailure.RelayNotConfigured), joinFailureMessage(PairingFailure.Unexpected))
        assertNotEquals(createFailureMessage(PairingFailure.RelayNotConfigured), createFailureMessage(PairingFailure.Unexpected))
        assertNotEquals(createFailureMessage(PairingFailure.InvalidRelay), createFailureMessage(PairingFailure.Unexpected))
    }

    @Test
    fun theWordsDifferWhereTheSituationsDiffer() {
        // The new device is told to get a new code from the other device; a code that cannot be read costs nothing.
        assertNotEquals(joinFailureMessage(PairingFailure.InvalidCode), joinFailureMessage(PairingFailure.WrongCode))
        assertNotEquals(joinFailureMessage(PairingFailure.Taken), joinFailureMessage(PairingFailure.Expired))
        assertNotEquals(joinFailureMessage(PairingFailure.Cancelled), joinFailureMessage(PairingFailure.UpdateRequired))
        assertNotEquals(createFailureMessage(PairingFailure.InstallTokenInvalid), createFailureMessage(PairingFailure.SpaceExists))
        assertNotEquals(inviteFailureMessage(PairingFailure.SpaceFull), inviteFailureMessage(PairingFailure.AnotherPairingOpen))
    }
}
