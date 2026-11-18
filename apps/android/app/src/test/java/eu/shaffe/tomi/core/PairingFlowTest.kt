package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Pairing on the fake core: a 6-digit code to check, then ONE yes/no answer about the history. */
@OptIn(ExperimentalCoroutinesApi::class)
class PairingFlowTest {
    private fun core() = FakeTomiCore(CoroutineScope(Job()), dispatchLatencyMs = 0, deliveryLatencyMs = 0)

    private suspend fun FakeTomiCore.pair(): PairingResult.AwaitingVerification =
        pairWithCode("7-crayon-renard") as PairingResult.AwaitingVerification

    @Test
    fun aSuccessfulRequestOnlyAsksForTheVerificationCodeAndJoinsNothing() = runTest {
        val core = core()
        val awaiting = core.pair()
        assertEquals(6, awaiting.verificationCode.digits.length)
        assertTrue(awaiting.verificationCode.digits.all { it.isDigit() })
        assertTrue(awaiting.historyItemCount > 0)
        assertFalse(core.isPaired.value)
    }

    @Test
    fun confirmingWithHistoryKeepsTheItemsAndJoinsTheSpace() = runTest {
        val core = core()
        val count = core.pair().historyItemCount
        core.confirmPairing(importHistory = true)
        assertTrue(core.isPaired.value)
        assertEquals(count, core.items.first().size)
    }

    @Test
    fun confirmingWithoutHistoryStartsFromAnEmptyFeed() = runTest {
        val core = core()
        core.pair()
        core.confirmPairing(importHistory = false)
        assertTrue(core.isPaired.value)
        assertTrue(core.items.first().isEmpty())
    }

    @Test
    fun cancellingTheVerificationJoinsNothingAndAConfirmationAfterwardsIsIgnored() = runTest {
        val core = core()
        core.pair()
        core.cancelPairing()
        core.confirmPairing(importHistory = true)
        assertFalse(core.isPaired.value)
    }

    @Test
    fun aMalformedCodeFailsBeforeAnyVerification() = runTest {
        val core = core()
        assertEquals(PairingResult.Failure(PairingFailure.InvalidCode), core.pairWithCode("ab"))
        assertFalse(core.isPaired.value)
    }

    @Test
    fun theFakeVerificationCodeIsAFixedOneAndPrintsRedacted() = runTest {
        val awaiting = core().pair()
        assertEquals("482913", awaiting.verificationCode.digits)
        assertFalse(awaiting.toString().contains("482"))
    }

    @Test
    fun anInvitationRunsFromTheCodeToTheNewDevice() = runTest {
        val core = core()
        core.pairWithCode("7-crayon-renard")
        core.confirmPairing(importHistory = true)
        val devicesBefore = core.devices.first().size

        val session = core.startInvitation()
        assertEquals("4821-crayon-renard", session.code()!!.reveal())
        assertEquals("https://relay.example.org/l/pair#4821-crayon-renard", session.link()!!.reveal())
        assertTrue(session.expiresInMs() > 0)

        val prompt = session.awaitPeer()
        assertEquals("482913", prompt.verificationCode.digits)
        session.confirm()
        val added = session.awaitDone()

        assertEquals(devicesBefore + 1, core.devices.first().size)
        assertTrue(core.devices.first().any { it.id == added.deviceId })
        assertNull("a finished session shows no code", session.code())
    }

    @Test
    fun cancellingAnInvitationShowsNoCodeAnyMore() = runTest {
        val session = core().startInvitation()
        session.cancel()
        assertNull(session.code())
        assertNull(session.link())
    }

    @Test
    fun theFounderStartsAnEmptySpaceWithTheNameItChose() = runTest {
        val core = core()
        core.createSpace(SensitiveText("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"), "Mon téléphone")
        assertTrue(core.isPaired.value)
        assertEquals("Mon téléphone", core.localDevice.value.name)
        assertTrue(core.items.first().isEmpty())
        assertEquals(1, core.devices.first().size)
    }

    @Test
    fun aBlankInstallTokenIsRefused() = runTest {
        val core = core()
        val failure = try {
            core.createSpace(SensitiveText("  "), "Pixel")
            null
        } catch (e: PairingException) {
            e.reason
        }
        assertEquals(PairingFailure.InstallTokenInvalid, failure)
        assertFalse(core.isPaired.value)
    }

    @Test
    fun aDeviceWithoutRelayNeedsOneToJoinOrToCreate() = runTest {
        val core = core().apply { relay = null }
        assertNull(core.relayUrl())
        assertEquals(PairingResult.Failure(PairingFailure.RelayNotConfigured), core.pairWithCode("7-crayon-renard"))
        assertEquals(PairingResult.Failure(PairingFailure.RelayNotConfigured), core.pairWithCode("7-crayon-renard", null))
        val failure = try {
            core.createSpace(SensitiveText("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"), "Pixel")
            null
        } catch (e: PairingException) {
            e.reason
        }
        assertEquals(PairingFailure.RelayNotConfigured, failure)
        assertFalse(core.isPaired.value)
    }

    @Test
    fun aRelayTypedNextToTheCodeIsTheOneTheVerificationScreenShows() = runTest {
        val core = core().apply { relay = null }
        val awaiting = core.pairWithCode("7-crayon-renard", "https://mon-relais.example.net:8443") as PairingResult.AwaitingVerification
        assertEquals("mon-relais.example.net:8443", awaiting.relayAuthority)
        // The device keeps no relay before it is confirmed.
        assertNull(core.relayUrl())
    }

    @Test
    fun theRelayOfTheDeviceIsShownWhenNoneIsTyped() = runTest {
        val awaiting = core().pair()
        assertEquals("relay.example.org", awaiting.relayAuthority)
    }
}
