package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import uniffi.tomi_core.FfiHistoryOffer
import uniffi.tomi_core.FfiJoinOutcome
import uniffi.tomi_core.ResumeKind
import uniffi.tomi_core.TomiException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * The pairing of the real core, on the JVM, against stand-ins of the UniFFI objects (NativeFakes.kt): what the adapter asks of the
 * core and in which order, how it reports what comes back, and that a cancelled coroutine ends the session.
 */
class RealTomiCorePairingTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @After
    fun tearDown() {
        scope.cancel()
    }

    private fun core(
        native: NativeFake,
        candidates: List<String?> = listOf("Pixel 8"),
        valid: (String) -> Boolean = { true },
        retry: List<Long> = emptyList(),
        resume: List<Long> = emptyList(),
    ) = RealTomiCore(native, scope, valid, { candidates }, Dispatchers.IO, retry, resume)

    private suspend fun failureOf(block: suspend () -> Unit): PairingFailure {
        try {
            block()
        } catch (e: PairingException) {
            return e.reason
        }
        fail("a PairingException was expected")
        throw AssertionError()
    }

    // --- Joining -------------------------------------------------------------------------------------------------------------

    @Test
    fun joiningShowsTheVerificationCodeOfTheCoreAndJoinsNothingYet() = runBlocking {
        val join = JoinFake(onWaitForInviter = { joinPrompt(history = FfiHistoryOffer(812u, 340_000_000u)) })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val awaiting = core(native).pairWithCode("4821-crayon-renard") as PairingResult.AwaitingVerification

        assertEquals(PairingPeer("PC-Bureau", DevicePlatform.Windows), awaiting.peer)
        assertEquals("482913", awaiting.verificationCode.digits)
        assertEquals("482 913", awaiting.verificationCode.grouped)
        assertEquals(812, awaiting.historyItemCount)
        assertEquals(340_000_000L, awaiting.historyBytes)
        assertEquals(3, awaiting.memberCount)
        assertEquals(90_000L, awaiting.expiresInMs)
        assertEquals(listOf("waitForInviter"), join.events)
        assertFalse("nothing is confirmed by the app on its own", join.events.any { it.startsWith("confirm") })
    }

    @Test
    fun theRelayOfThePairingIsHandedOverInFullToTheScreen() = runBlocking {
        val join = JoinFake(onWaitForInviter = { joinPrompt(relayAuthority = "mon-relais.example.net:8443") })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val awaiting = core(native).pairWithCode("4821-crayon-renard", "https://mon-relais.example.net:8443") as PairingResult.AwaitingVerification
        assertEquals("mon-relais.example.net:8443", awaiting.relayAuthority)
        // The relay that was typed goes to the core with the code; nothing is kept by the app.
        assertEquals("https://mon-relais.example.net:8443", native.lastJoinRelay)
    }

    @Test
    fun aLinkIsGivenWithoutAnotherRelayBecauseItNamesItsOwn() = runBlocking {
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> JoinFake() } }
        core(native).pairWithQr("https://relay.example.org/l/pair#4821-crayon-renard")
        assertNull(native.lastJoinRelay)
    }

    @Test
    fun theRelayOfTheDeviceIsReadFromTheCore() = runBlocking {
        assertNull(core(NativeFake()).relayUrl())
        assertEquals("https://relay.example.org", core(NativeFake().apply { deviceRelay = "https://relay.example.org" }).relayUrl())
    }

    @Test
    fun noHistoryOfferMeansNoHistoryQuestion() = runBlocking {
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> JoinFake(onWaitForInviter = { joinPrompt(history = null, members = null) }) } }
        val awaiting = core(native).pairWithCode("4821-crayon-renard") as PairingResult.AwaitingVerification
        assertEquals(0, awaiting.historyItemCount)
        assertNull(awaiting.memberCount)
    }

    @Test
    fun theDeviceIsCheckedAsPairedBeforeTheCodeIsGivenToTheCore() = runBlocking {
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> JoinFake() } }
        core(native).pairWithCode("4821-crayon-renard")
        val beforeJoin = native.calls.subList(0, native.calls.indexOf("joinSpace"))
        // One check when the core is built, one before the call.
        assertEquals(2, beforeJoin.count { it == "isPaired" })
    }

    @Test
    fun anAlreadyPairedDeviceNeverCallsJoinSpace() = runBlocking {
        val native = NativeFake().apply { paired = true }
        assertEquals(PairingResult.AlreadyPaired, core(native).pairWithCode("4821-crayon-renard"))
        assertFalse(native.calls.contains("joinSpace"))
    }

    @Test
    fun theNameGivenToTheCoreIsTheFirstCandidateItAccepts() = runBlocking {
        var given: Pair<String, String>? = null
        val native = NativeFake().apply { joinSpaceImpl = { code, name -> given = code to name; JoinFake() } }
        val core = core(native, candidates = listOf(null, "   ", "Mon   téléphone", "Pixel"), valid = { it != "Mon téléphone" })
        core.pairWithQr("https://relay.example.org/l/pair#4821-crayon-renard")
        // The first usable candidate is cleaned (one space) then refused by the rule of the core: the next one is used.
        assertEquals("https://relay.example.org/l/pair#4821-crayon-renard" to "Pixel", given)
    }

    @Test
    fun withoutAnyUsableNameTheDeviceIsCalledAndroid() = runBlocking {
        var given = ""
        val native = NativeFake().apply { joinSpaceImpl = { _, name -> given = name; JoinFake() } }
        core(native, candidates = listOf(null, ""), valid = { false }).pairWithCode("4821-crayon-renard")
        assertEquals("Android", given)
    }

    @Test
    fun anUnreadableCodeIsReportedOnlyOnceTheDeviceAndItsNameAreOutOfTheWay() = runBlocking {
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> throw TomiException.InvalidInput("unreadable") } }
        assertEquals(PairingResult.Failure(PairingFailure.InvalidCode), core(native).pairWithCode("zzz"))
    }

    @Test
    fun aPairingSettledMeanwhileIsNotACodeError() = runBlocking {
        lateinit var native: NativeFake
        native = NativeFake().apply {
            joinSpaceImpl = { _, _ ->
                native.paired = true
                throw TomiException.InvalidInput("already provisioned")
            }
        }
        val core = core(native)
        assertEquals(PairingResult.AlreadyPaired, core.pairWithCode("4821-crayon-renard"))
        assertTrue(core.isPaired.value)
    }

    @Test
    fun theFailuresOfTheCoreKeepTheirOwnMessages() = runBlocking {
        val cases = listOf(
            pairingError("pairing_failed") to PairingFailure.WrongCode,
            pairingError("pairing_expired") to PairingFailure.Expired,
            pairingError("pairing_taken") to PairingFailure.Taken,
            pairingError("pairing_cancelled") to PairingFailure.Cancelled,
            pairingError("pairing_unsupported") to PairingFailure.UpdateRequired,
            pairingError("relay_not_configured") to PairingFailure.RelayNotConfigured,
            TomiException.Network("offline") to PairingFailure.Network,
        )
        for ((error, expected) in cases) {
            val native = NativeFake().apply { joinSpaceImpl = { _, _ -> throw error } }
            assertEquals(error.message, PairingResult.Failure(expected), core(native).pairWithCode("4821-crayon-renard"))
        }
    }

    @Test
    fun aPanicOfTheCoreIsAnUnexpectedFailureNotACrash() = runBlocking {
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> throw uniffi.tomi_core.InternalException("panic") } }
        assertEquals(PairingResult.Failure(PairingFailure.Unexpected), core(native).pairWithCode("4821-crayon-renard"))

        val inviting = NativeFake().apply { paired = true; startInvitationImpl = { throw uniffi.tomi_core.InternalException("panic") } }
        assertEquals(PairingFailure.Unexpected, failureOf { core(inviting).startInvitation() })
    }

    @Test
    fun aSessionThatTheAppAlreadyReleasedIsACancelledPairing() = runBlocking {
        val join = JoinFake(onWaitForInviter = { throw IllegalStateException("JoinFake object has already been destroyed") })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        assertEquals(PairingResult.Failure(PairingFailure.Cancelled), core(native).pairWithCode("4821-crayon-renard"))

        val invite = InviteFake(onWaitForPeer = { throw IllegalStateException("InviteFake object has already been destroyed") })
        val inviting = NativeFake().apply { paired = true; startInvitationImpl = { invite } }
        val session = core(inviting).startInvitation()
        assertEquals(PairingFailure.Cancelled, failureOf { session.awaitPeer() })
    }

    @Test
    fun aWrongCodeEndsTheWaitAndReleasesTheSession() = runBlocking {
        val join = JoinFake(onWaitForInviter = { throw pairingError("pairing_failed") })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        assertEquals(PairingResult.Failure(PairingFailure.WrongCode), core(native).pairWithCode("4821-crayon-renard"))
        assertTrue(join.closed.await(5, TimeUnit.SECONDS))
    }

    @Test
    fun aConnectionThatKeepsBreakingAfterTheCodeWasUsedIsAnInterruptionNotARetry() = runBlocking {
        val join = JoinFake(onWaitForInviter = { throw TomiException.Network("offline") })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        // The code is spent once claimed: the user needs a new one, so this is not the plain "network" failure of a claim that failed.
        assertEquals(PairingResult.Failure(PairingFailure.Interrupted), core(native, retry = listOf(0L)).pairWithCode("4821-crayon-renard"))
        assertEquals(2, join.events.count { it == "waitForInviter" })
        assertTrue(join.closed.await(5, TimeUnit.SECONDS))
    }

    @Test
    fun aTransientErrorDuringTheWaitIsRetriedOnTheSameSession() = runBlocking {
        val attempts = AtomicInteger()
        val join = JoinFake(onWaitForInviter = {
            if (attempts.incrementAndGet() < 3) throw TomiException.Network("offline") else joinPrompt()
        })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val result = core(native, retry = listOf(0L, 0L)).pairWithCode("4821-crayon-renard")
        assertTrue(result is PairingResult.AwaitingVerification)
        assertEquals(3, join.events.count { it == "waitForInviter" })
    }

    // --- Confirming and cancelling -----------------------------------------------------------------------------------------

    @Test
    fun confirmingSendsTheAnswerThenWaitsForTheOtherPersonAndSettlesTheState() = runBlocking {
        lateinit var native: NativeFake
        val join = JoinFake(onWaitDone = {
            native.paired = true
            FfiJoinOutcome("device-1", "space-1", announcement)
        })
        native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val core = core(native)
        core.pairWithCode("4821-crayon-renard")
        assertFalse(core.isPaired.value)

        core.confirmPairing(importHistory = true)

        assertEquals(listOf("waitForInviter", "confirm(true)", "waitDone", "close"), join.events)
        assertTrue(core.isPaired.value)
        // Nothing is pending any more: a second confirmation does nothing.
        core.confirmPairing(importHistory = true)
        assertEquals(4, join.events.size)
    }

    @Test
    fun theHistoryAnswerIsPassedAsGiven() = runBlocking {
        val join = JoinFake()
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val core = core(native)
        core.pairWithCode("4821-crayon-renard")
        core.confirmPairing(importHistory = false)
        assertTrue(join.events.contains("confirm(false)"))
    }

    @Test
    fun aRefusalAfterTheConfirmationEndsThePairingWithItsReason() = runBlocking {
        val join = JoinFake(onWaitDone = { throw pairingError("pairing_cancelled") })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val core = core(native)
        core.pairWithCode("4821-crayon-renard")
        assertEquals(PairingFailure.Cancelled, failureOf { core.confirmPairing(importHistory = true) })
        assertTrue(join.closed.await(5, TimeUnit.SECONDS))
        assertFalse(core.isPaired.value)
    }

    @Test
    fun aTransientErrorOnConfirmKeepsTheSessionForANewTry() = runBlocking {
        val join = JoinFake(onConfirm = { throw TomiException.Network("offline") })
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val core = core(native)
        core.pairWithCode("4821-crayon-renard")
        assertEquals(PairingFailure.Interrupted, failureOf { core.confirmPairing(importHistory = true) })
        assertFalse("the session is still alive", join.events.contains("close"))
        core.cancelPairing()
        assertTrue(join.closed.await(5, TimeUnit.SECONDS))
    }

    @Test
    fun cancellingClosesTheChannelAndForgetsThePairing() = runBlocking {
        val join = JoinFake()
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val core = core(native)
        core.pairWithCode("4821-crayon-renard")

        core.cancelPairing()

        assertEquals(listOf("waitForInviter", "cancel", "close"), join.events)
        core.confirmPairing(importHistory = true)
        assertEquals("a confirmation after a cancellation is ignored", 3, join.events.size)
        core.cancelPairing()
        assertEquals("cancelling twice does nothing more", 3, join.events.size)
    }

    @Test
    fun cancellingFromAnotherCoroutineUnblocksTheWait() = runBlocking {
        val join = JoinFake().apply { blockUntilCancelled = true }
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val core = core(native)
        val pairing = async(Dispatchers.IO) { core.pairWithCode("4821-crayon-renard") }
        assertTrue(join.enteredWait.await(5, TimeUnit.SECONDS))

        core.cancelPairing()

        assertEquals(PairingResult.Failure(PairingFailure.Cancelled), withTimeout(5_000) { pairing.await() })
        assertTrue(join.closed.await(5, TimeUnit.SECONDS))
    }

    @Test
    fun cancellingTheCoroutineThatWaitsEndsTheSession() = runBlocking {
        val join = JoinFake().apply { blockUntilCancelled = true }
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> join } }
        val core = core(native)
        val job = launch(Dispatchers.IO) { core.pairWithCode("4821-crayon-renard") }
        assertTrue(join.enteredWait.await(5, TimeUnit.SECONDS))

        job.cancelAndJoin()

        assertTrue("the core was told to cancel", join.cancelled.await(5, TimeUnit.SECONDS))
        assertTrue("and the handle was released", join.closed.await(5, TimeUnit.SECONDS))
    }

    @Test
    fun aNewPairingClosesTheOneLeftOpen() = runBlocking {
        val first = JoinFake()
        val second = JoinFake()
        val sessions = ArrayDeque(listOf(first, second))
        val native = NativeFake().apply { joinSpaceImpl = { _, _ -> sessions.removeFirst() } }
        val core = core(native)
        core.pairWithCode("4821-crayon-renard")
        core.pairWithCode("4822-crayon-renard")
        assertTrue(first.events.contains("cancel"))
        assertTrue(first.events.contains("close"))
        assertFalse(second.events.contains("cancel"))
    }

    // --- Inviting ------------------------------------------------------------------------------------------------------------

    @Test
    fun anInvitationWrapsItsSecretsAndRunsTheSessionInOrder() = runBlocking {
        val invite = InviteFake()
        val native = NativeFake().apply { paired = true; startInvitationImpl = { invite } }
        val session = core(native).startInvitation()

        assertEquals("4821", session.channelId)
        assertEquals("4821-crayon-renard", session.code()!!.reveal())
        assertEquals("https://relay.example.org/l/pair#4821-crayon-renard", session.link()!!.reveal())
        assertEquals(120_000L, session.expiresInMs())
        assertFalse(session.code().toString().contains("crayon"))

        val prompt = session.awaitPeer()
        assertEquals(PairingPeer("Pixel", DevicePlatform.Android), prompt.peer)
        assertEquals("482913", prompt.verificationCode.digits)
        assertFalse(prompt.toString().contains("482"))

        session.confirm()
        val added = session.awaitDone()
        assertEquals(InvitedDevice("device-2", "Pixel", DevicePlatform.Android, historyAnnounced = false), added)
        assertEquals(listOf("waitForPeer", "confirm", "waitDone", "close"), invite.events)
        assertNull("a finished session shows no code", session.code())
    }

    @Test
    fun anAnnouncedHistoryIsReported() = runBlocking {
        val invite = InviteFake(onWaitDone = { uniffi.tomi_core.FfiInviteOutcome("device-2", "Pixel", "android", announcement) })
        val native = NativeFake().apply { paired = true; startInvitationImpl = { invite } }
        val session = core(native).startInvitation()
        session.awaitPeer()
        session.confirm()
        assertTrue(session.awaitDone().historyAnnounced)
    }

    @Test
    fun anInvitationThatCannotStartReportsWhy() = runBlocking {
        val cases = listOf(
            relayError(409, "too_many_devices") to PairingFailure.SpaceFull,
            relayError(409, "channel_limit") to PairingFailure.AnotherPairingOpen,
            TomiException.Network("offline") to PairingFailure.Network,
            relayError(429, "rate_limited") to PairingFailure.Network,
            TomiException.Blocked("Revoked") to PairingFailure.Blocked,
            TomiException.Blocked("UpdateRequired { update_url: None }") to PairingFailure.UpdateRequired,
        )
        for ((error, expected) in cases) {
            val native = NativeFake().apply { paired = true; startInvitationImpl = { throw error } }
            assertEquals(error.message, expected, failureOf { core(native).startInvitation() })
        }
    }

    @Test
    fun theCodeRunningOutEndsTheInvitationWithExpired() = runBlocking {
        val invite = InviteFake(onWaitForPeer = { throw pairingError("pairing_expired") })
        val native = NativeFake().apply { paired = true; startInvitationImpl = { invite } }
        val session = core(native).startInvitation()
        assertEquals(PairingFailure.Expired, failureOf { session.awaitPeer() })
        assertTrue(invite.closed.await(5, TimeUnit.SECONDS))
    }

    @Test
    fun cancellingAnInvitationClosesItOnceWhateverTheNumberOfCalls() = runBlocking {
        val invite = InviteFake()
        val native = NativeFake().apply { paired = true; startInvitationImpl = { invite } }
        val session = core(native).startInvitation()
        session.cancel()
        session.cancel()
        assertEquals(listOf("cancel", "close"), invite.events)
        assertNull(session.link())
    }

    @Test
    fun cancellingTheCoroutineThatWaitsForThePeerEndsTheInvitation() = runBlocking {
        val invite = InviteFake().apply { blockUntilCancelled = true }
        val native = NativeFake().apply { paired = true; startInvitationImpl = { invite } }
        val session = core(native).startInvitation()
        val job = launch(Dispatchers.IO) { session.awaitPeer() }
        assertTrue(invite.enteredWait.await(5, TimeUnit.SECONDS))

        job.cancelAndJoin()

        assertTrue(invite.cancelled.await(5, TimeUnit.SECONDS))
        assertTrue(invite.closed.await(5, TimeUnit.SECONDS))
    }

    // --- Creating the space ------------------------------------------------------------------------------------------------

    @Test
    fun theFounderHandsTheTokenAndTheNameToTheCoreOnceAndIsThenPaired() = runBlocking {
        lateinit var native: NativeFake
        var given: Pair<String, String>? = null
        native = NativeFake().apply {
            createSpaceImpl = { token, name ->
                given = token to name
                native.paired = true
            }
        }
        val core = core(native)
        core.createSpace(SensitiveText("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"), "Pixel 8")
        assertEquals("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns" to "Pixel 8", given)
        assertTrue(core.isPaired.value)
    }

    @Test
    fun theFounderGivesTheRelayOfTheLinkToTheCoreAndKeepsItOutOfTheToken() = runBlocking {
        val native = NativeFake().apply { createSpaceImpl = { _, _ -> } }
        core(native).createSpace(SensitiveText("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"), "Pixel 8", "https://mon-relais.example.net")
        assertEquals("https://mon-relais.example.net", native.lastCreateRelay)

        core(native).createSpace(SensitiveText("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"), "Pixel 8")
        assertNull(native.lastCreateRelay)
    }

    @Test
    fun theFoundersRefusalsKeepTheirOwnMessages() = runBlocking {
        val cases = listOf(
            relayError(403, "install_token_invalid") to PairingFailure.InstallTokenInvalid,
            relayError(409, "space_exists") to PairingFailure.SpaceExists,
            TomiException.Network("offline") to PairingFailure.Network,
            TomiException.InvalidInput("blank token") to PairingFailure.Unexpected,
            pairingError("relay_not_configured") to PairingFailure.RelayNotConfigured,
        )
        for ((error, expected) in cases) {
            val native = NativeFake().apply { createSpaceImpl = { _, _ -> throw error } }
            val core = core(native)
            assertEquals(error.message, expected, failureOf { core.createSpace(SensitiveText("token"), "Pixel") })
            assertFalse(core.isPaired.value)
        }
    }

    // --- The state follows the core ------------------------------------------------------------------------------------------

    @Test
    fun aChangeAnnouncedByTheCoreRefreshesTheStateAndTheFeed() = runBlocking {
        val native = NativeFake()
        val core = core(native)
        assertFalse(core.isPaired.value)

        val received = kotlinx.coroutines.channels.Channel<List<TomiItem>>(kotlinx.coroutines.channels.Channel.UNLIMITED)
        val collector = launch(Dispatchers.IO) { core.items.collect { received.send(it) } }
        assertTrue("the feed starts with what the core holds", withTimeout(5_000) { received.receive() }.isEmpty())

        native.paired = true
        native.itemsImpl = {
            listOf(
                uniffi.tomi_core.FfiItem(
                    "item-1",
                    uniffi.tomi_core.FfiItemContent.Readable(null, uniffi.tomi_core.TextContent("hello", null, null, uniffi.tomi_core.TextFormat.PLAIN), null, emptyList(), false),
                    "dev-pc", "PC-Bureau", 1_800_000_000_000L, null, uniffi.tomi_core.ReadPolicyKind.NORMAL, false, false, emptyList(), false, null, false, null,
                ),
            )
        }
        native.registeredListener!!.onChanged()

        assertEquals(listOf("item-1"), withTimeout(5_000) { received.receive() }.map { it.id })
        withTimeout(5_000) { core.isPaired.first { it } }
        assertEquals("Pixel 8", core.localDevice.value.name)
        collector.cancel()
    }

    // --- Settling a stopped pairing ----------------------------------------------------------------------------------------

    @Test
    fun nothingPendingMeansNoCallToTheRelay() = runBlocking {
        val native = NativeFake()
        assertEquals(ResumeOutcome.NothingPending, core(native).resumePairing())
        assertFalse(native.calls.contains("resumePairing"))
    }

    @Test
    fun aPairedDeviceIsNeverResumed() = runBlocking {
        val native = NativeFake().apply { paired = true; hasPendingImpl = { true } }
        assertEquals(ResumeOutcome.NothingPending, core(native).resumePairing())
        assertFalse(native.calls.contains("resumePairing"))
    }

    @Test
    fun aSettledPairingProvisionsTheDeviceAndRefreshesTheState() = runBlocking {
        lateinit var native: NativeFake
        native = NativeFake().apply {
            hasPendingImpl = { true }
            resumeImpl = {
                native.paired = true
                ResumeKind.PROVISIONED
            }
        }
        val core = core(native)
        assertFalse(core.isPaired.value)
        assertEquals(ResumeOutcome.Provisioned, core.resumePairing())
        assertTrue(core.isPaired.value)
    }

    @Test
    fun aBatchTheRelayDoesNotKnowIsDiscarded() = runBlocking {
        val native = NativeFake().apply { hasPendingImpl = { true }; resumeImpl = { ResumeKind.DISCARDED } }
        assertEquals(ResumeOutcome.Discarded, core(native).resumePairing())
    }

    @Test
    fun anOfflineRelayIsAskedAgain() = runBlocking {
        val attempts = AtomicInteger()
        val native = NativeFake().apply {
            hasPendingImpl = { true }
            resumeImpl = {
                if (attempts.incrementAndGet() < 3) throw TomiException.Network("offline") else ResumeKind.DISCARDED
            }
        }
        assertEquals(ResumeOutcome.Discarded, core(native, resume = listOf(0L, 0L)).resumePairing())
        assertEquals(3, attempts.get())
    }

    @Test
    fun aRelayThatStaysOfflineIsLeftForTheNextStart() = runBlocking {
        val attempts = AtomicInteger()
        val native = NativeFake().apply {
            hasPendingImpl = { true }
            resumeImpl = {
                attempts.incrementAndGet()
                throw TomiException.Network("offline")
            }
        }
        assertEquals(ResumeOutcome.Unreachable, core(native, resume = listOf(0L, 0L)).resumePairing())
        assertEquals("the first try and one per back-off step", 3, attempts.get())
    }

    @Test
    fun aRefusalIsNotRetried() = runBlocking {
        val attempts = AtomicInteger()
        val native = NativeFake().apply {
            hasPendingImpl = { true }
            resumeImpl = {
                attempts.incrementAndGet()
                throw TomiException.Blocked("Revoked")
            }
        }
        assertEquals(ResumeOutcome.Unreachable, core(native, resume = listOf(0L, 0L)).resumePairing())
        assertEquals(1, attempts.get())
    }
}
