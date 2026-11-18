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
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.tomi_core.FfiDeparture
import uniffi.tomi_core.FfiDepartureKind
import uniffi.tomi_core.FfiDepartureOutcome
import uniffi.tomi_core.FfiPendingReason
import uniffi.tomi_core.FfiRefusalReason
import uniffi.tomi_core.TomiException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The departure through the real adapter, on the JVM, against a stand-in of the UniFFI object (NativeFakes.kt): every outcome of the core
 * as the app's result, what the adapter forgets after an erasure, that the calls leave the caller's thread, that the pending departure is
 * re-read after each call, and that "Annuler" does not wait for an attempt in progress.
 */
class RealTomiCoreLeaveTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @After
    fun tearDown() {
        scope.cancel()
    }

    private fun core(native: NativeFake) = RealTomiCore(native, scope, { true }, { listOf("Pixel 8") }, Dispatchers.IO, emptyList(), emptyList())

    private fun pairedNative() = NativeFake().apply {
        paired = true
        itemsImpl = {
            listOf(
                uniffi.tomi_core.FfiItem(
                    "item-1",
                    uniffi.tomi_core.FfiItemContent.Readable(null, uniffi.tomi_core.TextContent("hello", null, null, uniffi.tomi_core.TextFormat.PLAIN), null, emptyList(), false),
                    "dev-pc", "PC-Bureau", 1_800_000_000_000L, null, uniffi.tomi_core.ReadPolicyKind.NORMAL, false, false, emptyList(), false, null, false, null,
                ),
            )
        }
    }

    /** What the real core does once it erased everything. */
    private fun NativeFake.erased() {
        paired = false
        departure = null
        itemsImpl = { throw TomiException.InvalidInput("not provisioned") }
    }

    @Test
    fun aProofUnpairsAndEmptiesTheFeedAtOnce() = runBlocking {
        var callerThread: Thread? = null
        val native = pairedNative().apply {
            leaveImpl = {
                callerThread = Thread.currentThread()
                erased()
                FfiDepartureOutcome.Erased
            }
        }
        val core = core(native)
        assertEquals(listOf("item-1"), core.items.first().map { it.id })

        assertEquals(DepartureResult.Erased, core.leaveSpace())

        assertFalse(core.isPaired.value)
        assertNull(core.departure.value)
        assertTrue("the old feed must not stay as 'the last feed read'", core.items.first().isEmpty())
        assertEquals("", core.localDevice.value.id)
        assertNotEquals("the blocking call must not run on the caller's thread", Thread.currentThread(), callerThread)
    }

    @Test
    fun everyOutcomeOfTheCoreIsTheSameResultAndOnlyAProofErases() = runBlocking {
        val cases = listOf(
            FfiDepartureOutcome.Pending(FfiPendingReason.NETWORK) to DepartureResult.Pending(PendingReason.Network),
            FfiDepartureOutcome.Pending(FfiPendingReason.SERVER) to DepartureResult.Pending(PendingReason.Server),
            FfiDepartureOutcome.Pending(FfiPendingReason.NOT_FOUND) to DepartureResult.Pending(PendingReason.NotFound),
            FfiDepartureOutcome.Pending(FfiPendingReason.NOT_SENT) to DepartureResult.Pending(PendingReason.NotSent),
            FfiDepartureOutcome.Refused(FfiRefusalReason.LAST_DEVICE) to DepartureResult.Refused(RefusalReason.LastDevice),
            FfiDepartureOutcome.Refused(FfiRefusalReason.UPDATE_REQUIRED) to DepartureResult.Refused(RefusalReason.UpdateRequired),
            FfiDepartureOutcome.Refused(FfiRefusalReason.RELAY_TOO_OLD) to DepartureResult.Refused(RefusalReason.RelayTooOld),
            FfiDepartureOutcome.Refused(FfiRefusalReason.UNEXPECTED) to DepartureResult.Refused(RefusalReason.Unexpected),
            FfiDepartureOutcome.Cancelled to DepartureResult.Cancelled,
            FfiDepartureOutcome.NothingPending to DepartureResult.NothingPending,
        )
        for ((outcome, expected) in cases) {
            val native = pairedNative().apply { leaveImpl = { outcome } }
            val core = core(native)

            assertEquals(outcome.toString(), expected, core.leaveSpace())

            assertTrue("nothing was erased", core.isPaired.value)
            assertEquals(listOf("item-1"), core.items.first().map { it.id })
            assertEquals("never retried by the adapter", 1, native.calls.count { it == "leaveSpace" })
            assertFalse(core.departureRunning.value)
        }
    }

    @Test
    fun whatTheCoreThrowsIsAFailureThatErasesNothing() = runBlocking {
        val errors = listOf(
            TomiException.Storage("disk"),
            TomiException.InvalidInput("not provisioned"),
            TomiException.Internal("panic"),
            uniffi.tomi_core.InternalException("boom"),
        )
        for (error in errors) {
            val native = pairedNative().apply { deleteSpaceImpl = { throw error } }
            val core = core(native)

            assertEquals(error.javaClass.simpleName, DepartureResult.Failed, core.deleteSpace())

            assertTrue(core.isPaired.value)
            assertEquals(listOf("item-1"), core.items.first().map { it.id })
        }
    }

    @Test
    fun eachCallGoesToItsOwnFunctionOfTheCore() = runBlocking {
        val native = pairedNative().apply {
            deleteSpaceImpl = { FfiDepartureOutcome.Pending(FfiPendingReason.SERVER) }
            retryImpl = { FfiDepartureOutcome.Pending(FfiPendingReason.NETWORK) }
            continueImpl = { FfiDepartureOutcome.Pending(FfiPendingReason.NOT_SENT) }
        }
        val core = core(native)

        assertEquals(DepartureResult.Pending(PendingReason.Server), core.deleteSpace())
        assertEquals(DepartureResult.Pending(PendingReason.Network), core.retryDeparture())
        assertEquals(DepartureResult.Pending(PendingReason.NotSent), core.continueDeparture())
        assertEquals(listOf("deleteSpace", "retryDeparture", "continueDeparture"), native.calls.filter { !it.startsWith("isPaired") })
    }

    @Test
    fun thePendingDepartureIsReReadAfterEachCall() = runBlocking {
        val native = pairedNative()
        val core = core(native)
        assertNull(core.departure.value)

        native.leaveImpl = {
            native.departure = FfiDeparture(FfiDepartureKind.LEAVE, false, 1_000L, FfiPendingReason.NETWORK)
            FfiDepartureOutcome.Pending(FfiPendingReason.NETWORK)
        }
        core.leaveSpace()
        assertEquals(Departure(DepartureKind.Leave, wiping = false, sinceMs = 1_000L, last = PendingReason.Network), core.departure.value)

        native.retryImpl = {
            native.departure = FfiDeparture(FfiDepartureKind.DELETE_SPACE, false, null, FfiPendingReason.NOT_FOUND)
            FfiDepartureOutcome.Pending(FfiPendingReason.NOT_FOUND)
        }
        core.retryDeparture()
        assertEquals(Departure(DepartureKind.DeleteSpace, wiping = false, sinceMs = null, last = PendingReason.NotFound), core.departure.value)

        native.cancelImpl = {
            native.departure = null
            true
        }
        assertTrue(core.cancelDeparture())
        assertNull(core.departure.value)
    }

    @Test
    fun aDepartureFoundAtStartUpIsShownBeforeAnyCall() {
        val native = pairedNative().apply { departure = FfiDeparture(FfiDepartureKind.LEAVE, true, 5L, null) }
        assertEquals(Departure(DepartureKind.Leave, wiping = true, sinceMs = 5L, last = null), core(native).departure.value)
    }

    @Test
    fun cancellingDoesNotWaitForTheAttemptInProgressWhichStaysRunningToItsEnd() = runBlocking {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val native = pairedNative().apply {
            leaveImpl = {
                entered.countDown()
                release.await(10, TimeUnit.SECONDS)
                FfiDepartureOutcome.Cancelled
            }
            cancelImpl = { true }
        }
        val core = core(native)

        val attempt = async(Dispatchers.Default) { core.leaveSpace() }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        assertTrue("an attempt runs", core.departureRunning.value)

        // Returns while the attempt is still blocked.
        assertTrue(withTimeout(2_000) { core.cancelDeparture() })
        assertTrue("the cancelled attempt is still running", core.departureRunning.value)

        release.countDown()
        assertEquals(DepartureResult.Cancelled, attempt.await())
        assertFalse(core.departureRunning.value)
        assertTrue("nothing was erased", core.isPaired.value)
    }

    @Test
    fun anAttemptRunsToItsEndEvenIfTheCallerIsCancelled() = runBlocking {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val native = pairedNative().apply {
            leaveImpl = {
                entered.countDown()
                release.await(10, TimeUnit.SECONDS)
                erased()
                FfiDepartureOutcome.Erased
            }
        }
        val core = core(native)

        val caller = scope.async { core.leaveSpace() }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        caller.cancel()
        release.countDown()

        // The proof that came is followed by the re-read: the app knows the device is erased.
        withTimeout(5_000) { core.isPaired.first { !it } }
        withTimeout(5_000) { core.departureRunning.first { !it } }
        Unit
    }

    @Test
    fun eraseWithoutProofErasesOnlyWhenTheCoreAgrees() = runBlocking {
        val allowed = pairedNative().apply {
            canEraseImpl = { true }
            eraseImpl = { erased() }
        }
        val core = core(allowed)
        core.items.first()
        assertTrue(core.canEraseWithoutProof())

        assertEquals(EraseResult.Erased, core.eraseWithoutProof())

        assertFalse(core.isPaired.value)
        assertTrue(core.items.first().isEmpty())
        assertFalse("never a departure call", allowed.calls.any { it == "leaveSpace" || it == "deleteSpace" })
    }

    @Test
    fun aRefusalOfTheCoreAfterTheBiometricCheckIsNotAllowedAndErasesNothing() = runBlocking {
        // The verdict expired, or another answer replaced it, while the user confirmed: the core answers InvalidInput.
        val native = pairedNative().apply { eraseImpl = { throw TomiException.InvalidInput("erasing without proof needs a recent answer") } }
        val core = core(native)

        assertEquals(EraseResult.NotAllowed, core.eraseWithoutProof())

        assertTrue(core.isPaired.value)
        assertEquals(listOf("item-1"), core.items.first().map { it.id })
    }

    @Test
    fun aStorageFailureOfTheErasureIsAFailure() = runBlocking {
        val native = pairedNative().apply { eraseImpl = { throw TomiException.Storage("disk") } }
        val core = core(native)

        assertEquals(EraseResult.Failed, core.eraseWithoutProof())
        assertTrue(core.isPaired.value)
    }

    @Test
    fun canEraseWithoutProofIsFalseWhenTheCoreFails() = runBlocking {
        val native = pairedNative().apply { canEraseImpl = { throw uniffi.tomi_core.InternalException("boom") } }
        assertFalse(core(native).canEraseWithoutProof())
    }

    @Test
    fun theSyncNoLongerRotatesFromTheApp() = runBlocking {
        val native = pairedNative().apply {
            syncImpl = { uniffi.tomi_core.FfiSyncReport(false, 0u, 0u, 0u, false, 0u, false, true) }
        }
        core(native).sync()
        assertFalse(native.calls.contains("maybeRotate"))
    }
}
