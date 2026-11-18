package eu.shaffe.tomi.ui.pairing

import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DevicePlatform
import eu.shaffe.tomi.core.FakeTomiCore
import eu.shaffe.tomi.core.InvitePrompt
import eu.shaffe.tomi.core.InviteSession
import eu.shaffe.tomi.core.InvitedDevice
import eu.shaffe.tomi.core.PairingException
import eu.shaffe.tomi.core.PairingFailure
import eu.shaffe.tomi.core.PairingPeer
import eu.shaffe.tomi.core.SensitiveText
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.core.TomiCoreException
import eu.shaffe.tomi.core.VerificationCode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The course of "Add a device": the code, its renewals, the comparison, the end. Scripted sessions, no Android, no network. */
@OptIn(ExperimentalCoroutinesApi::class)
class InviteFlowTest {
    private val peer = PairingPeer("Pixel", DevicePlatform.Android)

    /** One scripted invitation. */
    private class Session(
        private val onAwaitPeer: suspend () -> InvitePrompt,
        private val onAwaitDone: suspend () -> InvitedDevice = { InvitedDevice("device-2", "Pixel", DevicePlatform.Android, historyAnnounced = true) },
    ) : InviteSession {
        var confirms = 0
        override val channelId = "4821"

        override fun code() = SensitiveText("4821-crayon-renard")

        override fun link() = SensitiveText("https://relay.example.org/l/pair#4821-crayon-renard")

        override fun expiresInMs() = 120_000L

        override suspend fun awaitPeer() = onAwaitPeer()

        override suspend fun confirm() {
            confirms++
        }

        override suspend fun awaitDone() = onAwaitDone()

        override suspend fun cancel() = Unit
    }

    /** The fake core, except that the invitations are the scripted ones (or the errors) of [script], in order. */
    private class ScriptedCore(private val script: ArrayDeque<Any>) : TomiCore by FakeTomiCore(CoroutineScope(Job())) {
        var starts = 0
        var syncs = 0

        override suspend fun startInvitation(): InviteSession {
            starts++
            when (val next = script.removeFirst()) {
                is InviteSession -> return next
                is Throwable -> throw next
                else -> throw AssertionError("bad script")
            }
        }

        override suspend fun sync(pushToken: String?) {
            syncs++
        }
    }

    private fun prompt() = InvitePrompt(peer, VerificationCode("482 913"))

    private fun happy() = Session(onAwaitPeer = { prompt() })

    private fun expiring() = Session(onAwaitPeer = { throw PairingException(PairingFailure.Expired) })

    private class Harness(script: List<Any>) {
        val core = ScriptedCore(ArrayDeque(script))
        val states = mutableListOf<InviteUiState>()
        val discarded = mutableListOf<InviteSession>()
        val flow = InviteFlow(core, now = { 1_000L }, publish = { states += it }, discard = { discarded += it })
        val last: InviteUiState get() = states.last()
    }

    @Test
    fun theDeviceIsAddedOnceTheUserSaidSoAndTheHistoryExportIsStarted() = runTest {
        val session = happy()
        val h = Harness(listOf(session))
        val job = launch { h.flow.run() }
        runCurrent()

        val shown = h.states.filterIsInstance<InviteUiState.ShowCode>().single()
        assertEquals("4821-crayon-renard", shown.code.reveal())
        assertEquals(1_000L + 120_000L, shown.deadlineElapsedMs)
        // The new device said hello at once: the user is asked, and nothing is confirmed before the answer.
        assertTrue(h.last is InviteUiState.Verify)
        assertEquals(0, session.confirms)
        assertEquals("nothing is sent before the user said so", 0, h.core.syncs)

        h.flow.add()
        advanceUntilIdle()

        assertEquals(1, session.confirms)
        assertEquals(InviteUiState.Done(peer, historyAnnounced = true), h.last)
        assertEquals("the export of the history is started by a sync", 1, h.core.syncs)
        assertTrue(h.discarded.isEmpty())
        job.cancelAndJoin()
    }

    @Test
    fun theStatesFollowTheCourseOfTheInvitation() = runTest {
        val h = Harness(listOf(happy()))
        val job = launch { h.flow.run() }
        runCurrent()
        h.flow.add()
        advanceUntilIdle()

        assertEquals(
            listOf("Starting", "ShowCode", "Verify", "Adding", "Done"),
            h.states.map { it::class.simpleName },
        )
        job.cancelAndJoin()
    }

    @Test
    fun aCodeThatRanOutIsRenewedWithoutAnyNotice() = runTest {
        val h = Harness(listOf(expiring(), happy()))
        val job = launch { h.flow.run() }
        runCurrent()

        assertEquals(2, h.core.starts)
        val codes = h.states.filterIsInstance<InviteUiState.ShowCode>()
        assertEquals(2, codes.size)
        assertNull(codes[1].notice)
        assertTrue(h.last is InviteUiState.Verify)
        h.flow.abort()
        job.cancelAndJoin()
    }

    @Test
    fun aWrongCodeOrAnAbandonmentRenewsTheCodeAndSaysSo() = runTest {
        for (reason in listOf(PairingFailure.WrongCode, PairingFailure.Cancelled)) {
            val failing = Session(onAwaitPeer = { throw PairingException(reason) })
            val h = Harness(listOf(failing, happy()))
            val job = launch { h.flow.run() }
            runCurrent()

            val codes = h.states.filterIsInstance<InviteUiState.ShowCode>()
            assertEquals(reason.name, 2, codes.size)
            assertEquals(reason.name, R.string.invite_notice_failed, codes[1].notice)
            h.flow.abort()
            job.cancelAndJoin()
        }
    }

    @Test
    fun theCodeIsNotRenewedForeverWhileNothingHappens() = runTest {
        val h = Harness(List(InviteFlow.MAX_RENEWALS + 2) { expiring() })
        val job = launch { h.flow.run() }
        runCurrent()

        assertEquals(PairingFailure.Expired, (h.last as InviteUiState.Failed).reason)
        assertEquals("the first code and one renewal per allowed renewal, then the user decides", InviteFlow.MAX_RENEWALS + 1, h.core.starts)
        job.cancelAndJoin()
    }

    @Test
    fun aRefusalClosesTheChannelAndMakesANewCode() = runTest {
        val first = happy()
        val h = Harness(listOf(first, happy()))
        val job = launch { h.flow.run() }
        runCurrent()
        assertTrue(h.last is InviteUiState.Verify)

        h.flow.refuse()
        runCurrent()

        assertEquals("the refused invitation is closed at the relay", listOf<InviteSession>(first), h.discarded)
        assertEquals(0, first.confirms)
        val renewed = h.states.filterIsInstance<InviteUiState.ShowCode>().last()
        assertEquals(R.string.invite_notice_refused, renewed.notice)
        h.flow.abort()
        job.cancelAndJoin()
    }

    @Test
    fun leavingWhileComparingEndsTheCourseWithoutANewCode() = runTest {
        val first = happy()
        val h = Harness(listOf(first, happy()))
        val job = launch { h.flow.run() }
        runCurrent()
        assertTrue(h.last is InviteUiState.Verify)

        h.flow.abort()
        advanceUntilIdle()

        assertTrue("the run ended by itself", job.isCompleted)
        assertEquals("no new invitation was opened", 1, h.core.starts)
        assertEquals(listOf<InviteSession>(first), h.discarded)
    }

    @Test
    fun anInvitationThatCannotStartEndsWithItsReason() = runTest {
        for (reason in listOf(PairingFailure.SpaceFull, PairingFailure.AnotherPairingOpen, PairingFailure.Network)) {
            val h = Harness(listOf(PairingException(reason)))
            val job = launch { h.flow.run() }
            runCurrent()
            assertEquals(InviteUiState.Failed(reason), h.last)
            job.cancelAndJoin()
        }
        val other = Harness(listOf(TomiCoreException("storage")))
        val job = launch { other.flow.run() }
        runCurrent()
        assertEquals(InviteUiState.Failed(PairingFailure.Unexpected), other.last)
        job.cancelAndJoin()
    }

    @Test
    fun theOtherDeviceCancellingAfterTheConfirmationEndsTheInvitationWithoutSyncing() = runTest {
        val session = Session(onAwaitPeer = { prompt() }, onAwaitDone = { throw PairingException(PairingFailure.Cancelled) })
        val h = Harness(listOf(session))
        val job = launch { h.flow.run() }
        runCurrent()
        h.flow.add()
        advanceUntilIdle()

        assertEquals(InviteUiState.Failed(PairingFailure.Cancelled), h.last)
        assertEquals(0, h.core.syncs)
        job.cancelAndJoin()
    }

    @Test
    fun theStatesNeverPrintTheirSecrets() = runTest {
        val h = Harness(listOf(happy()))
        val job = launch { h.flow.run() }
        runCurrent()
        h.flow.add()
        advanceUntilIdle()

        for (state in h.states) {
            val printed = state.toString()
            assertFalse(printed, printed.contains("482"))
            assertFalse(printed, printed.contains("crayon"))
        }
        job.cancelAndJoin()
    }
}
