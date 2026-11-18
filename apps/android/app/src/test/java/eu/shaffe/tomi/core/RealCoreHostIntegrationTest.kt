package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.BeforeClass
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import uniffi.tomi_core.CoreConfig
import uniffi.tomi_core.RustTomiCore
import uniffi.tomi_core.isValidDeviceName
import java.io.File

/**
 * The generated bindings and [RealTomiCore] against the REAL Rust core, on the host: the library that the build makes for the desktop
 * (`generateUniffiKotlin`) is loaded through JNA, a database is opened in a temporary folder, and the calls cross the real FFI.
 *
 * It proves what the fakes of the other tests cannot: that the bindings and the library agree (UniFFI checks the signatures at load
 * time), that the errors of the core come out as the exceptions [PairingErrors] reads, and that nothing here needs a relay. The relay is
 * an address where nothing listens: a claim, a space creation and a sync fail with a network error, which is what is checked. A pairing
 * between two devices needs a relay and is not tested here (it is tested in crates/core, against an in-memory relay).
 *
 * Skipped when the host build of the core is not there.
 */
class RealCoreHostIntegrationTest {
    companion object {
        @JvmStatic
        @BeforeClass
        fun loadTheHostLibrary() {
            val library = System.getProperty("tomi.core.hostLibrary")?.let(::File)
            assumeTrue("host build of the Rust core not found (generateUniffiKotlin makes it)", library?.isFile == true)
            // Read when the bindings load the library for the first time: a path instead of the name `tomi_core`.
            System.setProperty("uniffi.component.tomi_core.libraryOverride", library!!.absolutePath)
        }

        /** Nothing listens here: the connection is refused at once, no DNS, no wait. */
        private const val NOWHERE = "https://127.0.0.1:1"
    }

    @get:Rule
    val folder = TemporaryFolder()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @After
    fun tearDown() {
        scope.cancel()
    }

    private fun open(relay: String? = NOWHERE): RealTomiCore {
        val config = CoreConfig(
            dbPath = File(folder.root, "tomi.db").path,
            filesDir = folder.newFolder("files").path,
            wrapKey = ByteArray(32) { (it + 1).toByte() },
            relayUrl = relay,
            appVersion = "0.0.0-test",
            platform = "android",
            autoImport = false,
        )
        return RealTomiCore(
            native = RustTomiCore.open(config),
            scope = scope,
            validDeviceName = ::isValidDeviceName,
            deviceNameCandidates = { listOf("Test phone") },
            resumeBackoffMs = emptyList(),
            retryDelaysMs = emptyList(),
        )
    }

    @Test
    fun anUnpairedDeviceOpensAndSaysSo() = runBlocking {
        val core = open()
        assertFalse(core.isPaired.value)
        assertEquals(SpaceState.Active, core.spaceState.value)
        assertEquals("", core.localDevice.value.id)
        assertTrue(core.items.first().isEmpty())
        assertTrue(core.devices.first().isEmpty())
    }

    @Test
    fun theRuleOfTheCoreDecidesWhichDeviceNamesAreValid() {
        val core = open()
        assertTrue(core.isValidDeviceName("Pixel 8 Phone"))
        assertFalse(core.isValidDeviceName(""))
        assertFalse(core.isValidDeviceName("   "))
        // The core trims the name it is given before judging it, and so does the function it exports.
        assertTrue(core.isValidDeviceName(" Pixel "))
        assertFalse(core.isValidDeviceName("Gal\u0007axy"))
        assertTrue(core.isValidDeviceName("a".repeat(64)))
        assertFalse(core.isValidDeviceName("a".repeat(65)))
    }

    @Test
    fun aCodeThatCannotBeReadIsAnInvalidCodeAndConsumesNothing() = runBlocking {
        val core = open()
        assertEquals(PairingResult.Failure(PairingFailure.InvalidCode), core.pairWithCode("not a code at all"))
        assertEquals(PairingResult.Failure(PairingFailure.InvalidCode), core.pairWithCode("4821-crayon"))
        assertEquals(PairingResult.Failure(PairingFailure.InvalidCode), core.pairWithQr("https://relay.example.org/l/pair?code=4821-crayon-renard"))
        assertEquals(PairingResult.Failure(PairingFailure.InvalidCode), core.pairWithQr("http://relay.example.org/l/pair#4821-crayon-renard"))
    }

    @Test
    fun aLinkOfAnotherRelayIsReadAndTriedThere() = runBlocking {
        // Any relay is accepted from a link (it is shown on the verification screen, not trusted): this one refuses the connection.
        val core = open(relay = null)
        assertEquals(PairingResult.Failure(PairingFailure.Network), core.pairWithQr("https://127.0.0.1:1/l/pair#4821-crayon-renard"))
        assertEquals(PairingResult.Failure(PairingFailure.Network), core.pairWithCode("4821-crayon-renard", "https://127.0.0.1:1"))
        assertNull("nothing is kept before the pairing succeeded", core.relayUrl())
    }

    @Test
    fun aReadableCodeWithoutARelayIsANetworkFailureNotACodeError() = runBlocking {
        val core = open()
        assertEquals(PairingResult.Failure(PairingFailure.Network), core.pairWithCode("4821-crayon-renard"))
        assertFalse(core.isPaired.value)
    }

    @Test
    fun aBuildWithoutRelayWaitsForOneFromTheLinkTheQrCodeOrTheField() = runBlocking {
        val core = open(relay = null)
        assertNull(core.relayUrl())
        assertEquals(PairingResult.Failure(PairingFailure.RelayNotConfigured), core.pairWithCode("4821-crayon-renard"))
        val e = try {
            core.createSpace(SensitiveText("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"), "Test phone")
            null
        } catch (e: PairingException) {
            e
        }
        assertEquals(PairingFailure.RelayNotConfigured, e?.reason)
        assertFalse(core.isPaired.value)
    }

    @Test
    fun theRelayOfTheBuildIsTheOneOfTheDevice() = runBlocking {
        assertEquals(NOWHERE, open().relayUrl())
    }

    @Test
    fun theCoreReadsTheLinksAndTheRelaysOfTheFieldsSharedByThePlatforms() {
        val links = CoreLinkReader
        val pair = links.parsePair("https://Relay.Example.org:8443/l/pair#4821-crayon-renard")!!
        assertEquals("https://relay.example.org:8443", pair.relayUrl)
        assertEquals("4821-crayon-renard", pair.code.reveal())
        assertNull(links.parsePair("tomi-share://pair#4821-crayon-renard")!!.relayUrl)
        assertNull(links.parsePair("http://relay.example.org/l/pair#4821-crayon-renard"))
        assertNull(links.parsePair("https://user@relay.example.org/l/pair#4821-crayon-renard"))

        val token = "y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47"
        val setup = links.parseSetup("https://relay.example.org/create#$token")!!
        assertEquals("https://relay.example.org", setup.relayUrl)
        assertEquals(token, setup.token.reveal())
        assertEquals(token, links.parseSetup("https://relay.example.org/create/$token")!!.token.reveal())
        assertNull(links.parseSetup("https://relay.example.org/create"))
        assertTrue(links.isInstallTokenShape(token))
        assertFalse(links.isInstallTokenShape("4821-crayon-renard"))

        assertEquals("https://relay.example.org", RelayAddress.canonical("relay.example.org", links))
        assertEquals("https://relay.example.org:8443", RelayAddress.canonical("https://relay.example.org:8443/", links))
        assertNull(RelayAddress.canonical("http://relay.example.org", links))
        assertNull(RelayAddress.canonical("not a relay", links))
    }

    @Test
    fun aDeviceThatIsNotInASpaceCannotInvite() = runBlocking {
        val core = open()
        val e = try {
            core.startInvitation()
            null
        } catch (e: PairingException) {
            e
        }
        assertEquals(PairingFailure.Unexpected, e?.reason)
    }

    @Test
    fun theFounderWithoutARelayGetsANetworkErrorAndKeepsItsKeysSealedForANewTry() = runBlocking {
        val core = open()
        val e = try {
            core.createSpace(SensitiveText("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"), "Test phone")
            null
        } catch (e: PairingException) {
            e
        }
        assertEquals(PairingFailure.Network, e?.reason)
        assertFalse(core.isPaired.value)
        // The keys were sealed for a retry: the next start has something to settle, and the relay cannot be asked.
        val outcome = core.resumePairing()
        assertNotEquals(ResumeOutcome.Provisioned, outcome)
    }

    @Test
    fun aFreshCoreHasNoDepartureAndNothingToErase() = runBlocking {
        // Proves that the bindings and the library agree on the functions of the departure (UniFFI checks the signatures at load).
        val core = open()
        assertEquals(DepartureResult.NothingPending, core.continueDeparture())
        assertEquals(null, core.departure.value)
        // A device that is not in a space has nothing to leave nor to erase, and nothing is sent to the relay for it.
        assertEquals(DepartureResult.Failed, core.leaveSpace())
        assertEquals(DepartureResult.Failed, core.retryDeparture())
        assertFalse(core.cancelDeparture())
        assertFalse(core.canEraseWithoutProof())
        assertEquals(EraseResult.NotAllowed, core.eraseWithoutProof())
        assertFalse(core.isPaired.value)
        assertFalse(core.departureRunning.value)
    }

    @Test
    fun aSyncOfAnUnpairedDeviceDoesNothing() = runBlocking {
        val core = open()
        core.sync()
        assertFalse(core.isPaired.value)
    }
}
