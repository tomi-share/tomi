package eu.shaffe.tomi.ui.devices

import eu.shaffe.tomi.core.FakeTomiCore
import eu.shaffe.tomi.core.TomiCore
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.IOException

/** Renaming and revoking a device offline must be a message, never a crash */
@OptIn(ExperimentalCoroutinesApi::class)
class DevicesViewModelTest {
    private val scope = CoroutineScope(Job())

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        scope.coroutineContext[Job]?.cancel()
    }

    /** The core whose device calls fail like a network error, or wait for the test. */
    private class OfflineCore(inner: FakeTomiCore) : TomiCore by inner {
        var revokeGate: CompletableDeferred<Unit>? = null
        var failures = 0

        override suspend fun renameDevice(id: String, name: String) {
            failures++
            throw IOException("offline")
        }

        override suspend fun revokeDevice(id: String) {
            revokeGate?.await()
            failures++
            throw IOException("offline")
        }
    }

    @Test
    fun `a rename that fails is reported and does not throw`() = runTest {
        val core = OfflineCore(FakeTomiCore(scope, startPaired = true))
        val viewModel = DevicesViewModel(core)

        viewModel.rename("dev-pc-bureau", "Autre nom")

        assertEquals(DevicesEvent.RenameFailed, viewModel.events.first())
        assertEquals(1, core.failures)
    }

    @Test
    fun `the stalled rotation of the core is what the screen shows`() = runTest {
        val fake = FakeTomiCore(scope, startPaired = true)
        val viewModel = DevicesViewModel(OfflineCore(fake))
        assertFalse(viewModel.rotationStalled.value)

        fake.setRotationStalled(true)
        assertTrue(viewModel.rotationStalled.value)

        fake.setRotationStalled(false)
        assertFalse(viewModel.rotationStalled.value)
    }

    @Test
    fun `a revocation that fails is reported, and the busy state ends`() = runTest {
        val core = OfflineCore(FakeTomiCore(scope, startPaired = true))
        val gate = CompletableDeferred<Unit>().also { core.revokeGate = it }
        val viewModel = DevicesViewModel(core)

        viewModel.revoke("dev-pc-bureau")
        assertTrue("busy while the key rotates", viewModel.busy.value)
        // A second tap on "Révoquer" meanwhile does not start a second revocation.
        viewModel.revoke("dev-pc-maison")
        gate.complete(Unit)

        assertEquals(DevicesEvent.RevokeFailed, viewModel.events.first())
        assertFalse(viewModel.busy.value)
        assertEquals(1, core.failures)
    }
}
