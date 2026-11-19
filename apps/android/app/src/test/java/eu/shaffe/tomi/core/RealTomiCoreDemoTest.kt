package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import uniffi.tomi_core.DemoConfig
import uniffi.tomi_core.TomiException
import java.util.concurrent.TimeUnit

/**
 * The way into the core's demo space through the real adapter, against a stand-in of the UniFFI object (NativeFakes.kt): when it is
 * offered, the configuration it passes, the state it leaves after the entry and after the exit, and a refusal of the core.
 */
class RealTomiCoreDemoTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @After
    fun tearDown() {
        scope.cancel()
    }

    private val config = DemoConfig(
        filesDir = "/data/tomi-core/demo",
        assetsDir = null,
        platform = "android",
        appVersion = "1.0",
        deviceName = "Galaxy S23",
        startPaired = true,
    )

    private fun core(native: NativeFake, demoConfig: (() -> DemoConfig)? = { config }) = RealTomiCore(
        native, scope, { true }, { listOf("Galaxy S23") }, Dispatchers.IO, emptyList(), emptyList(), demoConfig = demoConfig,
    )

    private fun demoItem() = uniffi.tomi_core.FfiItem(
        "demo-1",
        uniffi.tomi_core.FfiItemContent.Readable(null, uniffi.tomi_core.TextContent("hello", null, null, uniffi.tomi_core.TextFormat.PLAIN), null, emptyList(), false),
        "dev-pc", "PC-Bureau", 1_800_000_000_000L, null, uniffi.tomi_core.ReadPolicyKind.NORMAL, false, false, emptyList(), false, null, false, null,
    )

    /** What the core does when it opens its demo space: paired, in the demo, with a feed. */
    private fun NativeFake.opensTheDemo() {
        enterDemoImpl = { _ ->
            demo = true
            paired = true
            itemsImpl = { listOf(demoItem()) }
        }
    }

    @Test
    fun theEntryIsOfferedOnlyWithAConfigurationAndWhenTheCoreAllowsIt() {
        val native = NativeFake().apply { demoSupported = true }
        assertTrue(core(native).supportsDemoEntry())
        native.demoSupported = false
        assertFalse(core(native).supportsDemoEntry())
        assertFalse("no configuration, no demo", core(NativeFake().apply { demoSupported = true }, demoConfig = null).supportsDemoEntry())
    }

    @Test
    fun enteringPassesTheConfigurationAndPairsTheDeviceInTheDemo() = runBlocking {
        var passed: DemoConfig? = null
        val native = NativeFake().apply {
            demoSupported = true
            opensTheDemo()
            val open = enterDemoImpl
            enterDemoImpl = { passed = it; open(it) }
        }
        val core = core(native)
        assertFalse(core.isDemo.value)

        core.enterDemo()

        assertEquals("android", passed?.platform)
        assertTrue(core.isDemo.value)
        assertTrue(core.isPaired.value)
        assertEquals(listOf("demo-1"), core.items.first().map { it.id })
    }

    @Test
    fun enteringClosesAPendingPairingFirst() = runBlocking {
        val join = JoinFake().apply { blockUntilCancelled = true }
        val native = NativeFake().apply {
            joinSpaceImpl = { _, _ -> join }
            opensTheDemo()
        }
        val core = core(native)
        val pairing = async(Dispatchers.IO) { core.pairWithCode("4821-crayon-renard") }
        assertTrue(join.enteredWait.await(5, TimeUnit.SECONDS))

        core.enterDemo()

        assertTrue("the pairing was cancelled", join.cancelled.await(5, TimeUnit.SECONDS))
        assertTrue("and its handle released", join.closed.await(5, TimeUnit.SECONDS))
        assertTrue(withTimeout(5_000) { pairing.await() } is PairingResult.Failure)
        assertTrue(core.isDemo.value)
    }

    @Test
    fun aRefusalOfTheCoreIsATomiCoreExceptionAndChangesNothing() = runBlocking {
        val native = NativeFake().apply { enterDemoImpl = { throw TomiException.InvalidInput("already provisioned") } }
        val core = core(native)

        try {
            core.enterDemo()
            fail("the refusal must be thrown")
        } catch (e: TomiCoreException) {
            assertTrue(e.cause is TomiException.InvalidInput)
        }

        assertFalse(core.isDemo.value)
        assertFalse(core.isPaired.value)
    }

    @Test
    fun leavingUnpairsAndEmptiesTheFeed() = runBlocking {
        val native = NativeFake().apply {
            opensTheDemo()
            leaveDemoImpl = {
                demo = false
                paired = false
                itemsImpl = { throw TomiException.InvalidInput("not provisioned") }
                true
            }
        }
        val core = core(native)
        core.enterDemo()
        assertEquals(listOf("demo-1"), core.items.first().map { it.id })

        core.leaveDemo()

        assertFalse(core.isDemo.value)
        assertFalse(core.isPaired.value)
        assertTrue("the demo feed must not stay as 'the last feed read'", core.items.first().isEmpty())
        assertEquals(1, native.calls.count { it == "leaveDemo" })
    }

    @Test
    fun enteringWithoutAConfigurationThrows() = runBlocking {
        val native = NativeFake()
        val core = core(native, demoConfig = null)

        try {
            core.enterDemo()
            fail("no configuration, no demo")
        } catch (_: TomiCoreException) {
            // Expected.
        }

        assertFalse("the core was not asked", native.calls.contains("enterDemo"))
        assertFalse(core.isDemo.value)
    }

    @Test
    fun aDeviceThatBelongsToARealSpaceNeverEntersTheDemo() = runBlocking {
        val native = NativeFake().apply {
            demoSupported = true
            paired = true
            opensTheDemo()
        }
        val core = core(native)
        assertTrue(core.isPaired.value)
        assertFalse(core.isDemo.value)

        try {
            core.enterDemo()
            fail("a provisioned device must be refused")
        } catch (_: TomiCoreException) {
            // Expected.
        }

        assertFalse("the core was not even asked", native.calls.contains("enterDemo"))
        assertFalse(core.isDemo.value)
        assertTrue("the real space is untouched", core.isPaired.value)
    }
}
