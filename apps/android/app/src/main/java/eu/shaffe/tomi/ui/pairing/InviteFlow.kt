package eu.shaffe.tomi.ui.pairing

import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.InviteSession
import eu.shaffe.tomi.core.PairingException
import eu.shaffe.tomi.core.PairingFailure
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.core.TomiCoreException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred

/**
 * The course of "Add a device" (spec/pairing-v1.md 7.2, 4.2, 11), free of Android so that it can be tested: a code is shown and renewed,
 * the new device says hello, the user decides, the device is added. [InviteViewModel] runs it and shows the states it [publish]es.
 *
 * The code is renewed by itself while nothing happens (the relay drops an unused code after about two minutes), when the other device
 * gave up or typed a wrong code, and after a refusal, up to [MAX_RENEWALS] times in a row: then the user decides (the relay limits how
 * many channels a device may open, and a screen left open must not keep opening them).
 *
 * @param now `SystemClock.elapsedRealtime()`: the countdown of the code does not follow the clock of the phone.
 * @param discard closes a session at the relay without waiting, on a scope that outlives the screen.
 */
internal class InviteFlow(
    private val core: TomiCore,
    private val now: () -> Long,
    private val publish: (InviteUiState) -> Unit,
    private val discard: (InviteSession) -> Unit,
) {
    private var session: InviteSession? = null
    private var decision: CompletableDeferred<Boolean>? = null
    private var renewals = 0

    @Volatile
    private var aborted = false

    /** Runs until the device is added, or something ends the invitation; returns then (the last state is published). */
    suspend fun run() {
        renewals = 0
        aborted = false
        var notice: Int? = null
        while (!aborted) {
            publish(InviteUiState.Starting)
            val invitation = try {
                core.startInvitation()
            } catch (e: CancellationException) {
                throw e
            } catch (e: TomiCoreException) {
                publish(InviteUiState.Failed(reasonOf(e)))
                return
            }
            session = invitation
            val code = invitation.code()
            val link = invitation.link()
            if (code == null || link == null) {
                close()
                publish(InviteUiState.Failed(PairingFailure.Unexpected))
                return
            }
            publish(InviteUiState.ShowCode(code, link, now() + invitation.expiresInMs(), notice))
            notice = null

            val prompt = try {
                invitation.awaitPeer()
            } catch (e: CancellationException) {
                throw e
            } catch (e: TomiCoreException) {
                // The failed session released itself.
                session = null
                val reason = reasonOf(e)
                if (reason == PairingFailure.Expired || reason == PairingFailure.WrongCode || reason == PairingFailure.Cancelled) {
                    // The code ran out, or the other device gave up or typed a wrong code: a new code, while the screen stays open.
                    if (++renewals > MAX_RENEWALS) {
                        publish(InviteUiState.Failed(reason))
                        return
                    }
                    notice = if (reason == PairingFailure.Expired) null else R.string.invite_notice_failed
                    continue
                }
                publish(InviteUiState.Failed(reason))
                return
            }

            renewals = 0
            publish(InviteUiState.Verify(prompt.peer, prompt.verificationCode))
            val add = CompletableDeferred<Boolean>().also { decision = it }.await()
            decision = null
            if (aborted) {
                // The user left while comparing.
                close()
                return
            }
            if (!add) {
                // "They are different": nothing was given to anyone. The channel is closed and a new code is made.
                close()
                notice = R.string.invite_notice_refused
                continue
            }

            publish(InviteUiState.Adding(prompt.peer))
            try {
                invitation.confirm()
                val added = invitation.awaitDone()
                session = null
                publish(InviteUiState.Done(prompt.peer, added.historyAnnounced))
                syncAfterAdd()
            } catch (e: CancellationException) {
                throw e
            } catch (e: TomiCoreException) {
                session = null
                publish(InviteUiState.Failed(reasonOf(e)))
            }
            return
        }
    }

    /** "Add": the user compared the codes and they are the same. This alone gives the new device nothing: both people must confirm. */
    fun add() {
        decision?.complete(true)
    }

    /** "They are different", or back from the comparison. */
    fun refuse() {
        decision?.complete(false)
    }

    /** Closes the open invitation at the relay. The course of [run] is not touched: the caller cancels it, or it goes on with a new code. */
    fun close() {
        val open = session
        session = null
        if (open != null) discard(open)
    }

    /** The user leaves: nothing is waited for any more, no new code is made, and the open invitation is closed. */
    fun abort() {
        aborted = true
        decision?.complete(false)
        decision = null
        close()
    }

    /** The export of the history that the new device asked for is a task that the next sync starts ([InviteSession.awaitDone]). */
    private suspend fun syncAfterAdd() {
        try {
            core.sync()
        } catch (e: CancellationException) {
            throw e
        } catch (_: TomiCoreException) {
            // Offline: the next sync starts it.
        }
    }

    private fun reasonOf(e: TomiCoreException): PairingFailure = (e as? PairingException)?.reason ?: PairingFailure.Unexpected

    companion object {
        const val MAX_RENEWALS = 5
    }
}
