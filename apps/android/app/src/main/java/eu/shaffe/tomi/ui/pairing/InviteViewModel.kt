package eu.shaffe.tomi.ui.pairing

import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.InviteSession
import eu.shaffe.tomi.core.PairingFailure
import eu.shaffe.tomi.core.PairingPeer
import eu.shaffe.tomi.core.SensitiveText
import eu.shaffe.tomi.core.TomiCoreException
import eu.shaffe.tomi.core.VerificationCode
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.ui.common.Authenticator
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * What the "Add a device" screen shows. The code, the link and the verification code are wrapped ([SensitiveText], [VerificationCode]):
 * the state prints as redacted, and only the secure window of the screen draws them.
 */
sealed interface InviteUiState {
    /** The channel is being opened at the relay. */
    data object Starting : InviteUiState

    /** The code and the QR code are on screen, until [deadlineElapsedMs] (`SystemClock.elapsedRealtime()` time base). */
    data class ShowCode(
        val code: SensitiveText,
        val link: SensitiveText,
        val deadlineElapsedMs: Long,
        /** Why the code was renewed ("the pairing did not go through, a new code is ready"), or null. */
        val notice: Int? = null,
    ) : InviteUiState

    /** The new device said hello: the user compares the verification code and decides. */
    data class Verify(
        val peer: PairingPeer,
        val verificationCode: VerificationCode,
        /** The check of the user that follows "Ajouter cet appareil" was refused or cancelled: nothing was added. */
        val authDenied: Boolean = false,
    ) : InviteUiState

    /** "Add" was pressed: waiting for the person on the new device to confirm too. */
    data class Adding(val peer: PairingPeer) : InviteUiState

    data class Done(val peer: PairingPeer, val historyAnnounced: Boolean) : InviteUiState

    /** The fresh check of the user that opens "Add a device" was refused, cancelled or impossible (no screen lock set). */
    data class AuthRequired(val unavailable: Boolean) : InviteUiState

    data class Failed(val reason: PairingFailure) : InviteUiState
}

/** The message of a failure of an invitation. */
internal fun inviteFailureMessage(reason: PairingFailure): Int = when (reason) {
    PairingFailure.Expired -> R.string.invite_error_expired
    PairingFailure.Cancelled -> R.string.invite_error_cancelled
    PairingFailure.Network, PairingFailure.Interrupted -> R.string.pair_error_network
    PairingFailure.SpaceFull -> R.string.pair_error_space_full
    PairingFailure.AnotherPairingOpen -> R.string.invite_error_busy
    PairingFailure.UpdateRequired -> R.string.pair_error_update
    PairingFailure.Blocked -> R.string.pair_error_blocked
    PairingFailure.InvalidCode, PairingFailure.WrongCode, PairingFailure.Taken, PairingFailure.InstallTokenInvalid,
    PairingFailure.SpaceExists, PairingFailure.RelayNotConfigured, PairingFailure.InvalidRelay, PairingFailure.Unexpected,
    -> R.string.invite_error_failed
}

/**
 * "Add a device": this device is a member of the space and invites a new one. The course of it is [InviteFlow]; this class runs it on
 * the screen and shows its states.
 *
 * Scoped to its screen (the navigation entry), not to the activity: leaving the screen, by any way, ends the invitation ([onCleared]
 * closes the channel at the relay). A rotation of the phone does not.
 */
class InviteViewModel(private val container: AppContainer) : ViewModel() {
    var state by mutableStateOf<InviteUiState>(InviteUiState.Starting)
        private set

    /**
     * The prompt owner (the current activity), attached by the screen while it is shown and dropped when it goes: a view model must not
     * keep an activity alive. Without one (the screen is being recreated) the check counts as cancelled, never as passed.
     */
    @Volatile
    var authenticator: Authenticator? = null

    private val flow = InviteFlow(
        container.core,
        SystemClock::elapsedRealtime,
        { state = it },
        ::discard,
        authenticate = { authenticator?.forPairing() ?: AuthOutcome.Cancelled },
    )
    private var started = false
    private var job: Job? = null

    /** Called when the screen appears. Once: a rotation of the phone calls it again and must not restart anything. */
    fun start() {
        if (started) return
        started = true
        launchRun()
    }

    /** "Nouveau code" after a failure. */
    fun restart() {
        abort()
        launchRun()
    }

    /** "Add": the user compared the codes and they are the same. This alone gives the new device nothing: both people must confirm. */
    fun add() = flow.add()

    /** "They are different" or back from the comparison: the invitation is cancelled and a new code is made. */
    fun refuse() = flow.refuse()

    /** "Annuler", or leaving the screen: stops everything, closes the channel at the relay and wipes the session. */
    fun abort() {
        job?.cancel()
        job = null
        flow.abort()
    }

    override fun onCleared() {
        abort()
    }

    private fun launchRun() {
        job = viewModelScope.launch { flow.run() }
    }

    /** On the application's scope: the channel must be closed even though this view model may be going away. */
    private fun discard(open: InviteSession) {
        container.scope.launch {
            try {
                open.cancel()
            } catch (e: CancellationException) {
                throw e
            } catch (_: TomiCoreException) {
                // The channel then expires at the relay by itself.
            }
        }
    }
}
