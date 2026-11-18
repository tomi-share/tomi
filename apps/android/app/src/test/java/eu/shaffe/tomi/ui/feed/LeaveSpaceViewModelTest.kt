package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Departure
import eu.shaffe.tomi.core.DepartureFollowUp
import eu.shaffe.tomi.core.DepartureKind
import eu.shaffe.tomi.core.DepartureRefusal
import eu.shaffe.tomi.core.DepartureResult
import eu.shaffe.tomi.core.EraseResult
import eu.shaffe.tomi.core.FakeTomiCore
import eu.shaffe.tomi.core.PendingReason
import eu.shaffe.tomi.core.RefusalReason
import eu.shaffe.tomi.core.SpaceState
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.security.AuthOutcome
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * The departure as the UI shows it: one test per case of the table of spec/api-v1.md 7.9.3 as the
 * user sees it, the 2 s guard, the cancellation, and "Effacer quand même" refused unless the core allows it, before AND after the
 * biometric check. Virtual time: deterministic.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class LeaveSpaceViewModelTest {
    private val scope = CoroutineScope(Job())

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        (scope.coroutineContext[Job])?.cancel()
    }

    /** The core, whose departure calls wait for the test ([gate]) and are recorded. */
    private class ScriptedCore(private val inner: FakeTomiCore) : TomiCore by inner {
        val departureFlow = MutableStateFlow<Departure?>(null)
        val runningFlow = MutableStateFlow(false)
        val spaceFlow = MutableStateFlow(SpaceState.Active)
        override val departure: StateFlow<Departure?> = departureFlow
        override val departureRunning: StateFlow<Boolean> = runningFlow
        override val spaceState: StateFlow<SpaceState> = spaceFlow

        val calls = mutableListOf<String>()
        var gate = CompletableDeferred<DepartureResult>()
        var eraseGate = CompletableDeferred<EraseResult>()
        var canErase = false
        var cancelGives = true

        /** When set, [canEraseWithoutProof] waits for it (the screen can be re-created meanwhile). */
        var canEraseGate: CompletableDeferred<Unit>? = null

        private suspend fun call(name: String): DepartureResult {
            calls += name
            runningFlow.value = true
            try {
                return gate.await()
            } finally {
                runningFlow.value = false
            }
        }

        override suspend fun leaveSpace() = call("leaveSpace")
        override suspend fun deleteSpace() = call("deleteSpace")
        override suspend fun retryDeparture() = call("retryDeparture")
        override suspend fun continueDeparture() = call("continueDeparture")

        override suspend fun cancelDeparture(): Boolean {
            calls += "cancelDeparture"
            if (cancelGives) departureFlow.value = null
            return cancelGives
        }

        override suspend fun canEraseWithoutProof(): Boolean {
            calls += "canEraseWithoutProof"
            canEraseGate?.await()
            return canErase
        }

        override suspend fun eraseWithoutProof(): EraseResult {
            calls += "eraseWithoutProof"
            return eraseGate.await()
        }

        override suspend fun sync(pushToken: String?) {
            calls += "sync"
        }

        fun count(name: String) = calls.count { it == name }
    }

    private class RecordingFollowUp : DepartureFollowUp {
        var kicks = 0
        override val refusal = MutableStateFlow<DepartureRefusal?>(null)
        override fun kick() {
            kicks++
        }

        override fun consumeRefusal() {
            refusal.value = null
        }
    }

    private class Setup(val core: ScriptedCore, val followUp: RecordingFollowUp, val vm: LeaveSpaceViewModel)

    private fun setup(): Setup {
        val core = ScriptedCore(FakeTomiCore(scope, startPaired = true))
        val followUp = RecordingFollowUp()
        return Setup(core, followUp, LeaveSpaceViewModel(core, followUp))
    }

    private fun pending(kind: DepartureKind = DepartureKind.Leave, last: PendingReason? = null) =
        Departure(kind, wiping = false, sinceMs = 1_000L, last = last)

    /** "Se déconnecter" confirmed, the attempt running. */
    private fun Setup.leaving() {
        vm.open()
        vm.confirmLeave()
        core.departureFlow.value = pending()
    }

    /** Up to "Supprimer ton espace ?" armed and confirmed by biometrics, the attempt running. */
    private fun TestScope.deleting(s: Setup) {
        s.leaving()
        s.core.departureFlow.value = null
        s.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))
        s.core.gate = CompletableDeferred()
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertTrue(s.vm.beginDelete())
        s.vm.deleteAuthenticated()
    }

    // --- Proof, network, not found --------------------------------------------------------------------------------------------

    @Test
    fun aProofClosesTheDialog() = runTest {
        val s = setup()
        s.vm.open()
        assertEquals(LeaveUiState.ConfirmLeave(), s.vm.state.value)

        s.vm.confirmLeave()
        assertEquals(LeaveUiState.ConfirmLeave(busy = true), s.vm.state.value)
        s.core.gate.complete(DepartureResult.Erased)

        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
        assertEquals(1, s.core.count("leaveSpace"))
        assertEquals(0, s.followUp.kicks)
    }

    @Test
    fun confirmingTwiceCallsTheCoreOnce() = runTest {
        val s = setup()
        s.vm.open()
        s.vm.confirmLeave()
        s.vm.confirmLeave()
        assertEquals(1, s.core.count("leaveSpace"))
    }

    @Test
    fun aNetworkErrorLeavesTheBannerWithoutEraseAnywayAndStartsTheRetries() = runTest {
        for (reason in listOf(PendingReason.Network, PendingReason.Server)) {
            val s = setup()
            s.leaving()
            s.core.departureFlow.value = pending(last = reason)
            s.core.gate.complete(DepartureResult.Pending(reason))

            assertEquals(LeaveUiState.Hidden, s.vm.state.value)
            assertEquals(1, s.followUp.kicks)
            val banner = s.vm.banner.value as DepartureBannerState.Pending
            assertEquals(reason, banner.last)
            assertFalse("never on a network or server error", banner.offersEraseAnyway)
            assertTrue(s.core.isPaired.value)
        }
    }

    @Test
    fun notFoundLeavesTheBannerWithEraseAnywayAndNoRetry() = runTest {
        val s = setup()
        s.leaving()
        s.core.departureFlow.value = pending(last = PendingReason.NotFound)
        s.core.gate.complete(DepartureResult.Pending(PendingReason.NotFound))

        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
        assertEquals(0, s.followUp.kicks)
        assertTrue((s.vm.banner.value as DepartureBannerState.Pending).offersEraseAnyway)
        assertTrue("nothing was erased", s.core.isPaired.value)
    }

    @Test
    fun theSpaceNotFoundWithoutDepartureOffersEraseAnyway() = runTest {
        val s = setup()
        s.core.spaceFlow.value = SpaceState.NotFound
        assertEquals(DepartureBannerState.SpaceNotFound, s.vm.banner.value)
        assertTrue("the menu still offers the departure", s.vm.canOfferLeave.value)
    }

    // --- Dernier appareil, not_last_device, relais trop ancien -----------------------------------------------------------------

    @Test
    fun theLastDeviceIsAskedToDeleteTheSpaceWithAButtonArmedAfterTwoSeconds() = runTest {
        val s = setup()
        s.leaving()
        s.core.departureFlow.value = null
        s.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))
        assertEquals(LeaveUiState.ConfirmDelete(), s.vm.state.value)

        assertFalse("not before 2 s", s.vm.beginDelete())
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS - 1)
        assertFalse((s.vm.state.value as LeaveUiState.ConfirmDelete).armed)
        assertFalse(s.vm.beginDelete())
        advanceTimeBy(2)
        assertTrue((s.vm.state.value as LeaveUiState.ConfirmDelete).armed)

        s.core.gate = CompletableDeferred()
        assertTrue(s.vm.beginDelete())
        assertFalse("one prompt at a time", s.vm.beginDelete())
        assertEquals(0, s.core.count("deleteSpace"))
        s.vm.deleteAuthenticated()
        assertEquals(1, s.core.count("deleteSpace"))
        s.core.gate.complete(DepartureResult.Erased)
        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
    }

    @Test
    fun withoutStrongBiometricsNothingIsDeletedAndTheUserIsTold() = runTest {
        val s = setup()
        s.leaving()
        s.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertTrue(s.vm.beginDelete())

        s.vm.authEnded(unavailable = true)

        val state = s.vm.state.value as LeaveUiState.ConfirmDelete
        assertTrue(state.biometricUnavailable)
        assertFalse(state.authenticating)
        assertEquals(0, s.core.count("deleteSpace"))
    }

    @Test
    fun aCancelledBiometricPromptDeletesNothingAndSaysNothing() = runTest {
        val s = setup()
        s.leaving()
        s.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertTrue(s.vm.beginDelete())

        s.vm.authEnded(unavailable = false)

        val state = s.vm.state.value as LeaveUiState.ConfirmDelete
        assertFalse(state.biometricUnavailable)
        assertFalse(state.authenticating)
        assertTrue("the button works again", state.canConfirm)
        assertEquals(0, s.core.count("deleteSpace"))
    }

    @Test
    fun aStorageFailureOfTheDeletionIsShownAndTheButtonIsArmedAgain() = runTest {
        val s = setup()
        deleting(s)
        s.core.gate.complete(DepartureResult.Failed)

        assertEquals(LeaveUiState.ConfirmDelete(failed = true), s.vm.state.value)
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertTrue((s.vm.state.value as LeaveUiState.ConfirmDelete).canConfirm)
        assertTrue(s.core.isPaired.value)
    }

    @Test
    fun aDeletionThatMeetsTheNetworkLeavesTheBannerAndStartsTheRetries() = runTest {
        val s = setup()
        deleting(s)
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace, last = PendingReason.Network)
        s.core.gate.complete(DepartureResult.Pending(PendingReason.Network))

        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
        assertEquals(1, s.followUp.kicks)
        assertEquals(DepartureBannerState.Pending(DepartureKind.DeleteSpace, PendingReason.Network, running = false), s.vm.banner.value)
        assertTrue(s.core.isPaired.value)
    }

    @Test
    fun aRequestThatDidNotGetThroughOnlyVerifiesAndOffersNeitherRetriesNorEraseAnyway() = runTest {
        val s = setup()
        s.leaving()
        s.core.departureFlow.value = pending(last = PendingReason.NotSent)
        s.core.gate.complete(DepartureResult.Pending(PendingReason.NotSent))

        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
        assertEquals("no retry by itself", 0, s.followUp.kicks)
        val banner = s.vm.banner.value as DepartureBannerState.Pending
        assertEquals(PendingReason.NotSent, banner.last)
        assertFalse(banner.offersEraseAnyway)
        assertTrue(s.core.isPaired.value)
    }

    @Test
    fun notTheLastDeviceAfterAllEndsLikeADepartureOrAsksAgain() = runTest {
        // The core turns a 409 not_last_device into a departure (T3 then T1 + T2): its outcome is the one of the departure.
        val asked = setup()
        deleting(asked)
        asked.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))
        assertEquals("asked again, the button armed anew", LeaveUiState.ConfirmDelete(), asked.vm.state.value)
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertTrue((asked.vm.state.value as LeaveUiState.ConfirmDelete).armed)

        val left = setup()
        deleting(left)
        left.core.gate.complete(DepartureResult.Erased)
        assertEquals(LeaveUiState.Hidden, left.vm.state.value)
    }

    @Test
    fun aRelayTooOldOffersEraseAnyway() = runTest {
        val s = setup()
        deleting(s)
        s.core.gate.complete(DepartureResult.Refused(RefusalReason.RelayTooOld))
        assertEquals(RefusalReason.RelayTooOld, (s.vm.state.value as LeaveUiState.ConfirmDelete).error)

        s.core.canErase = true
        s.vm.openErase()

        val erase = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertFalse(erase.checking)
        assertFalse(erase.notAllowed)
        assertTrue(s.core.isPaired.value)
    }

    // --- Unexpected refusal, failure ---------------------------------------------------------------------------------------------

    @Test
    fun aRefusalOfTheRelayIsAFailureNeverADeparture() = runTest {
        for (reason in listOf(RefusalReason.Unexpected, RefusalReason.UpdateRequired)) {
            val leave = setup()
            leave.leaving()
            leave.core.gate.complete(DepartureResult.Refused(reason))
            assertEquals(reason.name, LeaveUiState.ConfirmLeave(error = reason), leave.vm.state.value)
            assertTrue(leave.core.isPaired.value)

            val delete = setup()
            deleting(delete)
            delete.core.gate.complete(DepartureResult.Refused(reason))
            assertEquals(reason.name, reason, (delete.vm.state.value as LeaveUiState.ConfirmDelete).error)
        }
    }

    @Test
    fun aStorageFailureIsShownAndCanBeRetried() = runTest {
        val s = setup()
        s.leaving()
        s.core.gate.complete(DepartureResult.Failed)
        assertEquals(LeaveUiState.ConfirmLeave(failed = true), s.vm.state.value)

        s.core.gate = CompletableDeferred()
        s.vm.confirmLeave()
        assertEquals(2, s.core.count("leaveSpace"))
    }

    // --- Annulation ---------------------------------------------------------------------------------------------------------

    @Test
    fun dismissIsIgnoredWhileTheAttemptRuns() = runTest {
        val s = setup()
        s.leaving()
        s.vm.dismiss()
        assertEquals(LeaveUiState.ConfirmLeave(busy = true), s.vm.state.value)
    }

    @Test
    fun cancellingIsImmediateTheAttemptStaysVisibleUntilItEndsAndItsResultIsDropped() = runTest {
        val s = setup()
        s.leaving()
        assertTrue(s.core.departureRunning.value)
        val events = mutableListOf<LeaveEvent>()
        val collector = backgroundScope.launchCollect(s.vm, events)

        s.vm.cancelRunning()

        assertEquals("closed at once", LeaveUiState.Hidden, s.vm.state.value)
        assertEquals(1, s.core.count("cancelDeparture"))
        assertEquals(listOf<LeaveEvent>(LeaveEvent.Cancelled(DepartureKind.Leave)), events)
        assertEquals("shown as running until it ends", DepartureBannerState.Cancelling, s.vm.banner.value)
        assertFalse("no new departure meanwhile", s.vm.canOfferLeave.value)

        // The attempt that had left ends with whatever the relay said: nothing is reopened.
        s.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))
        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
        assertEquals(DepartureBannerState.None, s.vm.banner.value)
        collector.cancel()
    }

    @Test
    fun cancelFromTheBannerWorksWhileAnAttemptRunsButNotDuringTheErasure() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(last = PendingReason.Network)
        s.core.runningFlow.value = true
        assertEquals(DepartureBannerState.Pending(DepartureKind.Leave, PendingReason.Network, running = true), s.vm.banner.value)

        s.vm.cancelFromBanner()
        assertEquals(1, s.core.count("cancelDeparture"))

        s.core.departureFlow.value = Departure(DepartureKind.Leave, wiping = true, sinceMs = 1L, last = null)
        assertEquals(DepartureBannerState.Wiping, s.vm.banner.value)
        s.vm.cancelFromBanner()
        assertFalse(s.vm.beginRetry())
        assertEquals(1, s.core.count("cancelDeparture"))
        assertEquals(0, s.core.count("retryDeparture"))
    }

    @Test
    fun aCancellationThatMissesTheMarkerOfARunningAttemptWaitsForItAndCancelsAgain() = runTest {
        val s = setup()
        s.core.cancelGives = false
        val events = mutableListOf<LeaveEvent>()
        val collector = backgroundScope.launchCollect(s.vm, events)
        s.vm.open()
        s.vm.confirmLeave()
        assertTrue("the attempt runs, its marker is not written yet", s.core.departureRunning.value)

        s.vm.cancelRunning()

        assertEquals("asked once, nothing to cancel yet", 1, s.core.count("cancelDeparture"))
        assertTrue("no confirmation while it is not cancelled", events.isEmpty())
        // The attempt writes its marker: the cancellation goes through, so the proof that may come erases nothing.
        s.core.cancelGives = true
        s.core.departureFlow.value = pending()
        assertEquals(2, s.core.count("cancelDeparture"))
        assertEquals(listOf<LeaveEvent>(LeaveEvent.Cancelled(DepartureKind.Leave)), events)
        collector.cancel()
    }

    @Test
    fun aCancellationOfAnAttemptThatEndsWithoutMarkerGivesNoConfirmation() = runTest {
        val s = setup()
        s.core.cancelGives = false
        val events = mutableListOf<LeaveEvent>()
        val collector = backgroundScope.launchCollect(s.vm, events)
        s.vm.open()
        s.vm.confirmLeave()
        s.vm.cancelRunning()

        s.core.gate.complete(DepartureResult.Refused(RefusalReason.Unexpected))

        assertEquals("not asked again: nothing is pending", 1, s.core.count("cancelDeparture"))
        assertTrue(events.isEmpty())
        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
        collector.cancel()
    }

    @Test
    fun theCancellationInProgressSurvivesANewViewModel() = runTest {
        // The user cancelled, then left the screen and came back (a new view model): the core still runs the attempt that had left.
        val s = setup()
        s.core.runningFlow.value = true

        val again = LeaveSpaceViewModel(s.core, s.followUp)

        assertEquals(DepartureBannerState.Cancelling, again.banner.value)
        assertFalse(again.canOfferLeave.value)
        s.core.runningFlow.value = false
        assertEquals(DepartureBannerState.None, again.banner.value)
    }

    // --- Resume at start-up, retry --------------------------------------------------------------------------------------

    @Test
    fun aDepartureFoundAtStartUpIsShown() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace)
        assertEquals(DepartureBannerState.Pending(DepartureKind.DeleteSpace, null, running = false), s.vm.banner.value)
        assertFalse(s.vm.canOfferLeave.value)
    }

    @Test
    fun retryingADeletionAsksForStrongBiometricsEveryTimeAndSendsNothingBeforeThem() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace)

        assertTrue("the prompt is asked", s.vm.beginRetry())
        assertFalse("a double tap changes nothing", s.vm.beginRetry())
        assertEquals("nothing is sent before the biometric check", 0, s.core.count("retryDeparture"))
        assertEquals(
            DepartureBannerState.Pending(DepartureKind.DeleteSpace, null, running = false, authenticating = true),
            s.vm.banner.value,
        )

        s.vm.retryAuthenticated()
        s.vm.retryAuthenticated()
        assertEquals(1, s.core.count("retryDeparture"))
        s.core.gate.complete(DepartureResult.Pending(PendingReason.Network))
        assertEquals(1, s.followUp.kicks)

        // Every new tap is a new gesture: the prompt again, never a memory of the previous one.
        s.core.gate = CompletableDeferred()
        assertTrue(s.vm.beginRetry())
        assertEquals(1, s.core.count("retryDeparture"))
    }

    @Test
    fun aCancelledPromptOfTheRetryOfADeletionSendsNothingAndCanBeAskedAgain() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace)
        val events = mutableListOf<LeaveEvent>()
        val collector = backgroundScope.launchCollect(s.vm, events)
        assertTrue(s.vm.beginRetry())

        s.vm.retryAuthEnded(unavailable = false)

        assertEquals(0, s.core.count("retryDeparture"))
        assertTrue("silent", events.isEmpty())
        assertEquals(DepartureBannerState.Pending(DepartureKind.DeleteSpace, null, running = false), s.vm.banner.value)
        assertTrue(s.vm.beginRetry())
        collector.cancel()
    }

    @Test
    fun withoutStrongBiometricsTheRetryOfADeletionSendsNothingAndTheUserIsTold() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace)
        val events = mutableListOf<LeaveEvent>()
        val collector = backgroundScope.launchCollect(s.vm, events)
        assertTrue(s.vm.beginRetry())

        s.vm.retryAuthEnded(unavailable = true)

        assertEquals(0, s.core.count("retryDeparture"))
        assertEquals(listOf<LeaveEvent>(LeaveEvent.BiometricUnavailable), events)
        collector.cancel()
    }

    @Test
    fun cancellingDuringThePromptOfTheRetryOfADeletionSendsNothing() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace)
        assertTrue(s.vm.beginRetry())

        s.vm.cancelFromBanner()
        assertEquals("\"Annuler\" works during the prompt", 1, s.core.count("cancelDeparture"))
        s.vm.retryAuthenticated()

        assertEquals("the deletion is gone: nothing to send", 0, s.core.count("retryDeparture"))
        assertFalse(s.vm.beginRetry())
    }

    @Test
    fun anAttemptStartedDuringThePromptOfTheRetryIsNotDoubled() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace)
        assertTrue(s.vm.beginRetry())
        s.core.runningFlow.value = true

        s.vm.retryAuthenticated()

        assertEquals(0, s.core.count("retryDeparture"))
    }

    @Test
    fun theRetryOfALeaveNeedsNoBiometricsAndIsSentOnce() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.Network)

        assertFalse("no prompt for a plain departure", s.vm.beginRetry())
        assertFalse(s.vm.beginRetry())
        assertEquals(1, s.core.count("retryDeparture"))
        s.core.gate.complete(DepartureResult.Pending(PendingReason.Network))
        assertEquals(1, s.followUp.kicks)
    }

    @Test
    fun aStorageFailureOfARetryIsReportedWhileTheDepartureIsStillPending() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.Network)
        val events = mutableListOf<LeaveEvent>()
        val collector = backgroundScope.launchCollect(s.vm, events)
        s.vm.beginRetry()

        s.core.gate.complete(DepartureResult.Failed)

        assertEquals(listOf<LeaveEvent>(LeaveEvent.Failed), events)
        collector.cancel()
    }

    @Test
    fun aFailureOfARetryThatWasCancelledMeanwhileIsNotNews() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.Network)
        val events = mutableListOf<LeaveEvent>()
        val collector = backgroundScope.launchCollect(s.vm, events)
        s.vm.beginRetry()
        s.vm.cancelFromBanner()

        s.core.gate.complete(DepartureResult.Failed)

        assertTrue("no \"nothing was erased, retry\" after a cancellation", events.none { it == LeaveEvent.Failed })
        collector.cancel()
    }

    @Test
    fun aRefusalOfTheRelayMetByARetryIsShownAsAFailure() = runTest {
        val unexpected = setup()
        unexpected.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.Network)
        unexpected.vm.beginRetry()
        unexpected.core.gate.complete(DepartureResult.Refused(RefusalReason.Unexpected))
        assertEquals(LeaveUiState.ConfirmLeave(error = RefusalReason.Unexpected), unexpected.vm.state.value)

        val last = setup()
        last.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.Network)
        last.vm.beginRetry()
        last.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))
        assertEquals(LeaveUiState.ConfirmDelete(), last.vm.state.value)
    }

    @Test
    fun aRefusalMetByAnAutomaticAttemptIsShownAsAFailure() = runTest {
        val unexpected = setup()
        unexpected.followUp.refusal.value = DepartureRefusal(DepartureKind.Leave, RefusalReason.Unexpected)
        assertEquals(LeaveUiState.ConfirmLeave(error = RefusalReason.Unexpected), unexpected.vm.state.value)
        assertNull("shown once", unexpected.followUp.refusal.value)

        val last = setup()
        last.followUp.refusal.value = DepartureRefusal(DepartureKind.Leave, RefusalReason.LastDevice)
        assertEquals(LeaveUiState.ConfirmDelete(), last.vm.state.value)
    }

    @Test
    fun anAutomaticRefusalWaitsWhileADialogIsOpenAndIsShownWhenItCloses() = runTest {
        val s = setup()
        s.vm.open()

        s.followUp.refusal.value = DepartureRefusal(DepartureKind.Leave, RefusalReason.Unexpected)

        assertEquals("the open dialog is not replaced", LeaveUiState.ConfirmLeave(), s.vm.state.value)
        assertEquals("kept for later", DepartureRefusal(DepartureKind.Leave, RefusalReason.Unexpected), s.followUp.refusal.value)

        s.vm.dismiss()

        assertEquals(LeaveUiState.ConfirmLeave(error = RefusalReason.Unexpected), s.vm.state.value)
        assertNull("shown once", s.followUp.refusal.value)
    }

    @Test
    fun aRevokedDeviceIsOfferedToLeave() = runTest {
        val s = setup()
        s.core.spaceFlow.value = SpaceState.Revoked
        assertEquals(DepartureBannerState.Revoked, s.vm.banner.value)
        s.vm.open()
        assertEquals(LeaveUiState.ConfirmLeave(), s.vm.state.value)
    }

    // --- "Effacer quand même" -----------------------------------------------------------------------------------------------

    /** Opens D4 with the core allowing it, the box checked and the button armed. */
    private fun TestScope.eraseReady(s: Setup) {
        s.core.canErase = true
        s.vm.openErase()
        s.vm.setEraseChecked(true)
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).canConfirm)
    }

    @Test
    fun eraseAnywayIsRefusedAfterANetworkError() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(last = PendingReason.NotFound)
        s.core.canErase = false
        s.vm.openErase()
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).checking)
        // The relay is asked again (continue_departure: within 15 minutes of the gesture the core sends the request again, later it only
        // verifies), and answers with a network error.
        assertEquals(1, s.core.count("continueDeparture"))
        s.core.gate.complete(DepartureResult.Pending(PendingReason.Network))

        val state = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertTrue(state.notAllowed)
        assertFalse(state.checking)
        s.vm.setEraseChecked(true)
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertFalse(s.vm.beginErase())
        assertEquals(0, s.core.count("eraseWithoutProof"))
        assertTrue(s.core.isPaired.value)
    }

    @Test
    fun withoutAPendingDepartureTheCheckIsASync() = runTest {
        val s = setup()
        s.core.spaceFlow.value = SpaceState.NotFound
        s.vm.openErase()
        assertEquals(1, s.core.count("sync"))
        assertEquals(0, s.core.count("continueDeparture"))
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).notAllowed)
    }

    @Test
    fun eraseAnywayNeedsTheBoxAndTwoSeconds() = runTest {
        val s = setup()
        s.core.canErase = true
        s.vm.openErase()
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS + 1)
        assertFalse("the box is not checked", s.vm.beginErase())

        val again = setup()
        again.core.canErase = true
        again.vm.openErase()
        again.vm.setEraseChecked(true)
        advanceTimeBy(LeaveSpaceViewModel.ARM_DELAY_MS - 1)
        assertFalse("not before 2 s", again.vm.beginErase())
        advanceTimeBy(2)
        assertTrue(again.vm.beginErase())
        assertEquals(0, again.core.count("eraseWithoutProof"))
    }

    @Test
    fun eraseAnywayReadsTheCoreAgainRightBeforeTheBiometricPrompt() = runTest {
        val s = setup()
        eraseReady(s)
        // The verdict expired between the opening of the dialog and the tap.
        s.core.canErase = false

        assertFalse(s.vm.beginErase())

        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).notAllowed)
        assertEquals(2, s.core.count("canEraseWithoutProof"))
        assertEquals(0, s.core.count("eraseWithoutProof"))
    }

    @Test
    fun aRefusalOfTheCoreAfterTheBiometricCheckErasesNothingAndSaysWhy() = runTest {
        val s = setup()
        eraseReady(s)
        assertTrue(s.vm.beginErase())
        s.vm.eraseAuthenticated()
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).busy)
        s.vm.dismiss()
        assertTrue("dismiss is ignored while erasing", s.vm.state.value is LeaveUiState.EraseWithoutProof)

        s.core.eraseGate.complete(EraseResult.NotAllowed)

        val state = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertTrue(state.notAllowed)
        assertTrue(state.expired)
        assertFalse(state.busy)
        assertTrue("nothing was erased", s.core.isPaired.value)
    }

    @Test
    fun eraseAnywayErasesOnlyAfterTheBiometricCheck() = runTest {
        val s = setup()
        eraseReady(s)
        assertTrue(s.vm.beginErase())
        assertEquals(0, s.core.count("eraseWithoutProof"))
        s.vm.authEnded(unavailable = false)
        assertEquals("a cancelled prompt erases nothing", 0, s.core.count("eraseWithoutProof"))

        assertTrue(s.vm.beginErase())
        s.vm.eraseAuthenticated()
        s.core.eraseGate.complete(EraseResult.Erased)
        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
        assertEquals(1, s.core.count("eraseWithoutProof"))
    }

    @Test
    fun eraseAnywayWaitsWhileAnAttemptRuns() = runTest {
        val s = setup()
        eraseReady(s)
        s.core.runningFlow.value = true
        assertFalse(s.vm.beginErase())
    }

    @Test
    fun anAttemptThatStartsDuringTheBiometricPromptIsWaitedForAndTheDialogCanBeLeft() = runTest {
        val s = setup()
        eraseReady(s)
        assertTrue(s.vm.beginErase())
        // An automatic attempt starts while the prompt is up and holds the core.
        s.core.runningFlow.value = true

        s.vm.eraseAuthenticated()

        val waiting = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertTrue(waiting.busy)
        assertTrue(waiting.waiting)
        assertEquals("the core is not asked while the attempt runs", 0, s.core.count("eraseWithoutProof"))

        s.vm.dismiss()
        assertEquals("\"Annuler\" leaves, nothing was erased", LeaveUiState.Hidden, s.vm.state.value)
        s.core.runningFlow.value = false
        assertEquals("not asked once the attempt is over either", 0, s.core.count("eraseWithoutProof"))
        assertTrue(s.core.isPaired.value)
    }

    @Test
    fun theWaitedForAttemptEndsAndTheErasureGoesOn() = runTest {
        val s = setup()
        eraseReady(s)
        assertTrue(s.vm.beginErase())
        s.core.runningFlow.value = true
        s.vm.eraseAuthenticated()

        s.core.runningFlow.value = false

        assertEquals(1, s.core.count("eraseWithoutProof"))
        assertFalse((s.vm.state.value as LeaveUiState.EraseWithoutProof).waiting)
        s.core.eraseGate.complete(EraseResult.Erased)
        assertEquals(LeaveUiState.Hidden, s.vm.state.value)
    }

    @Test
    fun aStorageFailureOfTheErasureIsShownAndNothingIsErased() = runTest {
        val s = setup()
        eraseReady(s)
        assertTrue(s.vm.beginErase())
        s.vm.eraseAuthenticated()

        s.core.eraseGate.complete(EraseResult.Failed)

        val state = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertTrue(state.failed)
        assertFalse(state.busy)
        assertFalse(state.notAllowed)
        assertTrue(s.core.isPaired.value)
    }

    @Test
    fun withoutStrongBiometricsEraseAnywayErasesNothingAndTheUserIsTold() = runTest {
        val s = setup()
        eraseReady(s)
        assertTrue(s.vm.beginErase())

        s.vm.authEnded(unavailable = true)

        val state = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertTrue(state.biometricUnavailable)
        assertFalse(state.authenticating)
        assertEquals(0, s.core.count("eraseWithoutProof"))
    }

    @Test
    fun aScreenRecreatedWhileTheCoreIsAskedDoesNotLeaveThePromptLocked() = runTest {
        val s = setup()
        eraseReady(s)
        s.core.canEraseGate = CompletableDeferred()
        val job = launch(Dispatchers.Main) { s.vm.beginErase() }
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).authenticating)

        job.cancelAndJoin()

        val state = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertFalse("the scope of the dialog is gone: nobody will end the prompt", state.authenticating)
        assertTrue("the user can confirm again", state.canConfirm)
    }

    @Test
    fun theNotAllowedVerdictCanBeCheckedAgain() = runTest {
        val s = setup()
        s.core.spaceFlow.value = SpaceState.NotFound
        s.vm.openErase()
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).notAllowed)
        assertEquals(1, s.core.count("sync"))

        // Still not allowed: the relay is asked again, and the verdict stays.
        s.vm.recheckErase()
        assertEquals("the relay is asked again", 2, s.core.count("sync"))
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).notAllowed)
        assertFalse((s.vm.state.value as LeaveUiState.EraseWithoutProof).checking)

        // The core now allows it (the verdict is read first, no need to ask the relay).
        s.core.canErase = true
        s.vm.recheckErase()

        val state = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertFalse(state.notAllowed)
        assertFalse(state.checking)
        assertEquals(2, s.core.count("sync"))
    }

    @Test
    fun aRefusalMetWhileCheckingEraseAnywayEndsTheDepartureAndIsShown() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.NotFound)
        s.vm.openErase()
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).checking)

        s.core.gate.complete(DepartureResult.Refused(RefusalReason.Unexpected))

        assertEquals(LeaveUiState.ConfirmLeave(error = RefusalReason.Unexpected), s.vm.state.value)
        assertEquals(0, s.core.count("eraseWithoutProof"))
        assertTrue(s.core.isPaired.value)
    }

    @Test
    fun aLastDeviceMetWhileCheckingEraseAnywayOffersTheDeletion() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.NotFound)
        s.vm.openErase()

        s.core.gate.complete(DepartureResult.Refused(RefusalReason.LastDevice))

        assertTrue(s.vm.state.value is LeaveUiState.ConfirmDelete)
    }

    @Test
    fun aRelayTooOldMetWhileCheckingKeepsEraseAnywayOpenAndAllowed() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.DeleteSpace, last = PendingReason.NotFound)
        s.vm.openErase()
        // The refusal "relay too old" is what allows this dialog: the core says so when read again.
        s.core.canErase = true

        s.core.gate.complete(DepartureResult.Refused(RefusalReason.RelayTooOld))

        val state = s.vm.state.value as LeaveUiState.EraseWithoutProof
        assertFalse(state.checking)
        assertFalse(state.notAllowed)
    }

    @Test
    fun aNetworkErrorMetWhileCheckingEraseAnywayStartsTheRetries() = runTest {
        val s = setup()
        s.core.departureFlow.value = pending(DepartureKind.Leave, last = PendingReason.NotFound)
        s.vm.openErase()

        s.core.gate.complete(DepartureResult.Pending(PendingReason.Network))

        assertEquals(1, s.followUp.kicks)
        assertTrue((s.vm.state.value as LeaveUiState.EraseWithoutProof).notAllowed)
    }

    // --- The biometric prompt (composable) ---------------------------------------------------------------------------------------

    @Test
    fun aPromptCutShortByARecreatedScreenCountsAsCancelled() = runTest {
        val ended = mutableListOf<Boolean>()
        var succeeded = 0
        val job = launch(Dispatchers.Main) {
            strongPrompt(prompt = { awaitCancellation() }, onSuccess = { succeeded++ }, onEnded = { ended += it })
        }

        job.cancelAndJoin()

        assertEquals("cancelled, not unavailable", listOf(false), ended)
        assertEquals(0, succeeded)
    }

    @Test
    fun aPromptTellsExactlyHowItEnded() = runTest {
        // One assertion per outcome: only Success runs the action, only Unavailable says "unavailable".
        val expected = mapOf(
            AuthOutcome.Success to ("success" to emptyList<Boolean>()),
            AuthOutcome.Unavailable to ("ended" to listOf(true)),
            AuthOutcome.Cancelled to ("ended" to listOf(false)),
            AuthOutcome.Failed to ("ended" to listOf(false)),
        )
        assertEquals("every outcome is covered", AuthOutcome.entries.toSet(), expected.keys)
        for ((outcome, want) in expected) {
            val ended = mutableListOf<Boolean>()
            var succeeded = 0
            strongPrompt(prompt = { outcome }, onSuccess = { succeeded++ }, onEnded = { ended += it })

            assertEquals("$outcome runs the action", if (want.first == "success") 1 else 0, succeeded)
            assertEquals("$outcome ends the prompt", want.second, ended)
        }
    }

    private fun CoroutineScope.launchCollect(vm: LeaveSpaceViewModel, into: MutableList<LeaveEvent>) =
        launch(Dispatchers.Main) { vm.events.collect { into += it } }
}
