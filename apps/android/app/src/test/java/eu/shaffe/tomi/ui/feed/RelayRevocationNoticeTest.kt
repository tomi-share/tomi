package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.FakeTomiCore
import eu.shaffe.tomi.core.RelayRevocation
import eu.shaffe.tomi.core.RelayRevocationVia
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The wording of the notice for a device the relay removed, and the fake core's controllable state for the two notices. */
class RelayRevocationNoticeTest {
    @Test
    fun eachCauseHasItsOwnMessage() {
        val admin = RelayRevocationNotice.messageRes(RelayRevocationVia.Admin)
        val self = RelayRevocationNotice.messageRes(RelayRevocationVia.Self)
        val other = RelayRevocationNotice.messageRes(RelayRevocationVia.Other)
        assertEquals(R.string.snackbar_relay_revoked_admin, admin)
        assertEquals(R.string.snackbar_relay_revoked_self, self)
        assertEquals(R.string.snackbar_relay_revoked_other, other)
        assertEquals(3, setOf(admin, self, other).size)
    }

    @Test
    fun theFakeCoreDeliversARevocationOnceAndKeepsItUntilSomeoneCollects() = runTest {
        val scope = CoroutineScope(Job())
        try {
            val core = FakeTomiCore(scope, startPaired = true)
            core.relayRevoked("dev-pc", "PC-Bureau", RelayRevocationVia.Admin)
            core.relayRevoked("dev-tab", "Tablette", RelayRevocationVia.Self)

            assertEquals(RelayRevocation("dev-pc", "PC-Bureau", RelayRevocationVia.Admin), core.relayRevocations.first())
            assertEquals(RelayRevocation("dev-tab", "Tablette", RelayRevocationVia.Self), core.relayRevocations.first())
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun theFakeCoreRotationStallIsControllableAndFalseByDefault() = runTest {
        val scope = CoroutineScope(Job())
        try {
            val core = FakeTomiCore(scope, startPaired = true)
            assertFalse("never shown when false", core.rotationStalled.value)
            core.setRotationStalled(true)
            assertTrue(core.rotationStalled.value)
            core.setRotationStalled(false)
            assertFalse(core.rotationStalled.value)
        } finally {
            scope.cancel()
        }
    }
}
