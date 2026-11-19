package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.tomi_core.FfiRelayRevocation

/**
 * The two notices about the removal of a device, through the real adapter against a stand-in of the UniFFI object: the persisted
 * "rotation stalled" flag, and the revocations the relay alone made, handed over once by the core.
 */
class RealTomiCoreNoticesTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @After
    fun tearDown() {
        scope.cancel()
    }

    private fun core(native: NativeFake) = RealTomiCore(native, scope, { true }, { listOf("Pixel 8") }, Dispatchers.IO, emptyList(), emptyList())

    private fun pairedNative() = NativeFake().apply {
        paired = true
        syncImpl = { uniffi.tomi_core.FfiSyncReport(false, 0u, 0u, 0u, false, 0u, false, false) }
    }

    /** The core announces a change and the adapter re-reads in the background: waits for [check] (a few seconds at most). */
    private fun eventually(check: () -> Boolean) {
        val deadline = System.nanoTime() + 5_000_000_000L
        while (!check()) {
            check(System.nanoTime() < deadline) { "the adapter did not re-read in time" }
            Thread.sleep(10)
        }
    }

    @Test
    fun theStalledRotationIsReadAtStartAndOnEveryChange() {
        val native = pairedNative().apply { rotationStalledNow = true }
        val core = core(native)
        assertTrue("persisted flag read at start", core.rotationStalled.value)

        native.rotationStalledNow = false
        native.registeredListener!!.onChanged()
        eventually { !core.rotationStalled.value }

        native.rotationStalledNow = true
        native.registeredListener!!.onChanged()
        eventually { core.rotationStalled.value }
    }

    @Test
    fun theStalledRotationIsReadAfterASyncToo() = runBlocking {
        val native = pairedNative()
        val core = core(native)
        assertFalse(core.rotationStalled.value)

        native.rotationStalledNow = true
        core.sync()

        eventually { core.rotationStalled.value }
    }

    @Test
    fun aRelayRevocationIsDeliveredOnceWithItsCause() = runBlocking {
        val native = pairedNative().apply {
            pendingRelayRevocations += FfiRelayRevocation("dev-pc", "PC-Bureau", "admin")
            pendingRelayRevocations += FfiRelayRevocation("dev-tab", "Tablette", "self")
            pendingRelayRevocations += FfiRelayRevocation("dev-x", "dev-x", "something-new")
        }
        val core = core(native)

        val shown = withTimeout(5_000) { core.relayRevocations.take(3).toList() }
        assertEquals(
            listOf(
                RelayRevocation("dev-pc", "PC-Bureau", RelayRevocationVia.Admin),
                RelayRevocation("dev-tab", "Tablette", RelayRevocationVia.Self),
                RelayRevocation("dev-x", "dev-x", RelayRevocationVia.Other),
            ),
            shown,
        )

        // A later change announces nothing new: the core handed each one over once, and the adapter does not repeat it.
        native.registeredListener!!.onChanged()
        native.pendingRelayRevocations += FfiRelayRevocation("dev-late", "Late", "admin")
        native.registeredListener!!.onChanged()
        assertEquals("dev-late", withTimeout(5_000) { core.relayRevocations.first() }.deviceId)
    }

    @Test
    fun aRevocationTakenBeforeAnyoneListensIsKept() = runBlocking {
        val native = pairedNative()
        val core = core(native)
        native.pendingRelayRevocations += FfiRelayRevocation("dev-pc", "PC-Bureau", "admin")
        core.sync()

        // Nobody was collecting when it was taken: it waits for the collector instead of being lost.
        assertEquals("dev-pc", withTimeout(5_000) { core.relayRevocations.first() }.deviceId)
    }

    @Test
    fun nothingIsTakenOutsideASpace() {
        val native = NativeFake().apply {
            pendingRelayRevocations += FfiRelayRevocation("dev-pc", "PC-Bureau", "admin")
        }
        core(native)

        assertEquals("the core keeps it for the day this device is in a space", 1, native.pendingRelayRevocations.size)
    }
}
