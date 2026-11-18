package eu.shaffe.tomi.ui.feed

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.Departure
import eu.shaffe.tomi.core.DepartureFollowUp
import eu.shaffe.tomi.core.DepartureKind
import eu.shaffe.tomi.core.DepartureRefusal
import eu.shaffe.tomi.core.DepartureResult
import eu.shaffe.tomi.core.EraseResult
import eu.shaffe.tomi.core.PendingReason
import eu.shaffe.tomi.core.RefusalReason
import eu.shaffe.tomi.core.SpaceState
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.core.isTransient
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.ui.common.rememberAuthenticator
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

/** Which dialog of the departure is open, and what it shows. */
sealed interface LeaveUiState {
    data object Hidden : LeaveUiState

    /**
     * "Se déconnecter de ton espace ?". [busy]: the attempt runs (progress bar, only "Annuler"). [error]: the relay refused for good (a
     * failure, never a departure). [failed]: the storage refused the erasure or the call failed; nothing was erased.
     */
    data class ConfirmLeave(val busy: Boolean = false, val error: RefusalReason? = null, val failed: Boolean = false) : LeaveUiState

    /**
     * "Supprimer ton espace ?", after the relay said this is the only device. [armed]: the confirm button works 2 s after the dialog shows
     * (against an accidental tap). [authenticating]: the strong biometric prompt is up. [error] [RefusalReason.RelayTooOld] offers
     * "Effacer quand même".
     */
    data class ConfirmDelete(
        val armed: Boolean = false,
        val authenticating: Boolean = false,
        val busy: Boolean = false,
        val error: RefusalReason? = null,
        val failed: Boolean = false,
        val biometricUnavailable: Boolean = false,
    ) : LeaveUiState {
        val canConfirm: Boolean get() = armed && !authenticating && !busy
    }

    /**
     * "Effacer quand même" (D4): erases this device without a proof of the relay. [checking]: Tomi asks the core (and, if needed, the
     * relay) whether it is allowed. [notAllowed]: it is not (the button is off); [expired]: it was, but no longer after the biometric check.
     * The confirm button needs the [checked] box and 2 s ([armed]).
     */
    data class EraseWithoutProof(
        val checked: Boolean = false,
        val armed: Boolean = false,
        val checking: Boolean = false,
        val authenticating: Boolean = false,
        val busy: Boolean = false,
        val notAllowed: Boolean = false,
        val expired: Boolean = false,
        val failed: Boolean = false,
        val biometricUnavailable: Boolean = false,
        /** The biometric check passed, but an attempt of the departure still holds the core: waits for it ("Annuler" closes the dialog). */
        val waiting: Boolean = false,
    ) : LeaveUiState {
        val canConfirm: Boolean get() = checked && armed && !checking && !authenticating && !busy && !notAllowed
    }
}

/** The banner at the top of the feed: what the departure is doing, at all times. */
sealed interface DepartureBannerState {
    data object None : DepartureBannerState

    /**
     * A departure waits for a proof of the relay. [running]: an attempt runs right now ("Réessayer" is off, "Annuler" is not).
     * [authenticating]: the strong biometric prompt of the "Réessayer" of a deletion is up (the same buttons are off but "Annuler").
     */
    data class Pending(
        val kind: DepartureKind,
        val last: PendingReason?,
        val running: Boolean,
        val authenticating: Boolean = false,
    ) : DepartureBannerState {
        /** Only when the relay said "not found", never after a network or server error. */
        val offersEraseAnyway: Boolean get() = last == PendingReason.NotFound && !running && !authenticating
    }

    /** The proof came: the erasure runs. No button. */
    data object Wiping : DepartureBannerState

    /** Cancelled, but the attempt that had left keeps running to its end (it erases nothing). */
    data object Cancelling : DepartureBannerState

    /** No departure, and the relay does not find the space ("espace introuvable"). */
    data object SpaceNotFound : DepartureBannerState

    /** No departure, and this device was removed from the space by another one. */
    data object Revoked : DepartureBannerState
}

/** One-line confirmations of the departure, shown by the feed. */
sealed interface LeaveEvent {
    data class Cancelled(val kind: DepartureKind) : LeaveEvent

    /** A "Réessayer" from the banner failed in the storage: nothing was erased. */
    data object Failed : LeaveEvent

    /** The "Réessayer" of a deletion needs strong biometrics and the phone has none enrolled: nothing was sent. */
    data object BiometricUnavailable : LeaveEvent
}

/**
 * No marker and an attempt runs while no dialog is open: that is an attempt whose departure was just cancelled (the dialog closes when it
 * is cancelled), still going to its end. Derived from the core, so that it survives a re-creation of the view model.
 */
internal fun bannerOf(
    departure: Departure?,
    running: Boolean,
    authenticating: Boolean,
    dialogHidden: Boolean,
    spaceState: SpaceState,
): DepartureBannerState = when {
    departure?.wiping == true -> DepartureBannerState.Wiping
    departure != null -> DepartureBannerState.Pending(departure.kind, departure.last, running, authenticating)
    running && dialogHidden -> DepartureBannerState.Cancelling
    spaceState == SpaceState.NotFound -> DepartureBannerState.SpaceNotFound
    spaceState == SpaceState.Revoked -> DepartureBannerState.Revoked
    else -> DepartureBannerState.None
}

/**
 * The departure of this device (spec/api-v1.md 7.9): the dialogs, the banner and their buttons.
 *
 * The core erases only on a proof of the relay; everything else leaves a pending departure that the banner shows with "Réessayer" and
 * "Annuler", and that [followUp] retries by itself. The one way to erase without proof is "Effacer quand même": a checkbox, 2 s, a fresh
 * look at whether the core allows it, then strong biometrics (asked by the composable, which has the activity), then the call; a refusal
 * of the core after the biometric check is shown as such and erases nothing. When the device is erased it is no longer paired and the
 * app restarts on the welcome screen (TomiRoot).
 *
 * Nothing here is logged (UniffiConfinementTest).
 */
class LeaveSpaceViewModel(
    private val core: TomiCore,
    private val followUp: DepartureFollowUp,
    private val armDelayMs: Long = ARM_DELAY_MS,
) : ViewModel() {
    private val dialog = MutableStateFlow<LeaveUiState>(LeaveUiState.Hidden)
    val state: StateFlow<LeaveUiState> = dialog.asStateFlow()

    /** Where the "Réessayer" of the banner is: a second tap does nothing until it is [Idle] again. */
    private enum class RetryPhase { Idle, Authenticating, Running }

    private val retryPhase = MutableStateFlow(RetryPhase.Idle)

    /** An attempt (or an erasure) runs in the core: "Effacer ce téléphone" waits for it to end. */
    val attemptRunning: StateFlow<Boolean> = core.departureRunning

    val banner: StateFlow<DepartureBannerState> =
        combine(core.departure, core.departureRunning, retryPhase, dialog, core.spaceState) { d, running, retry, shown, s ->
            bannerOf(d, running || retry == RetryPhase.Running, retry == RetryPhase.Authenticating, shown == LeaveUiState.Hidden, s)
        }
            .stateIn(
                viewModelScope,
                SharingStarted.Eagerly,
                bannerOf(core.departure.value, core.departureRunning.value, false, true, core.spaceState.value),
            )

    /** "Se déconnecter" belongs in the menu only when nothing is pending nor running (the banner handles the rest). */
    val canOfferLeave: StateFlow<Boolean> =
        combine(core.departure, core.departureRunning) { departure, running -> departure == null && !running }
            .stateIn(viewModelScope, SharingStarted.Eagerly, core.departure.value == null && !core.departureRunning.value)

    private val eventChannel = Channel<LeaveEvent>(Channel.BUFFERED)
    val events: Flow<LeaveEvent> = eventChannel.receiveAsFlow()

    /** Changes whenever the user closes or replaces a dialog: a result or a timer started for the previous one is dropped. */
    private var generation = 0
    private var armJob: Job? = null

    init {
        // A refusal met by an automatic attempt (start-up, back-off) is shown as soon as no dialog is open.
        viewModelScope.launch {
            combine(followUp.refusal, dialog) { refusal, shown -> refusal.takeIf { shown == LeaveUiState.Hidden } }
                .filterNotNull()
                .collect { refusal ->
                    followUp.consumeRefusal()
                    showRefusal(refusal)
                }
        }
    }

    // --- "Se déconnecter" ---------------------------------------------------------------------------------------------------

    /** "Se déconnecter" in the menu, or in the banner of a revoked device. */
    fun open() {
        if (dialog.value == LeaveUiState.Hidden) replace(LeaveUiState.ConfirmLeave())
    }

    /** Back, tap outside, "Annuler" / "Fermer" of an idle dialog. Ignored while a call runs: "Annuler" is then [cancelRunning]. */
    fun dismiss() {
        val busy = when (val current = dialog.value) {
            LeaveUiState.Hidden -> return
            is LeaveUiState.ConfirmLeave -> current.busy
            is LeaveUiState.ConfirmDelete -> current.busy || current.authenticating
            // Waiting for an attempt to end after the biometric check: nothing has been asked of the core yet, "Annuler" leaves.
            is LeaveUiState.EraseWithoutProof -> (current.busy || current.authenticating) && !current.waiting
        }
        if (!busy) replace(LeaveUiState.Hidden)
    }

    /** "Se déconnecter" in the dialog (also the retry after a refusal or a failure). One attempt at a time. */
    fun confirmLeave() {
        val current = dialog.value as? LeaveUiState.ConfirmLeave ?: return
        if (current.busy) return
        dialog.value = LeaveUiState.ConfirmLeave(busy = true)
        attempt(DepartureKind.Leave) { core.leaveSpace() }
    }

    /** "Annuler" while an attempt of the dialog runs: immediate, never erases. The attempt goes on to its end and erases nothing. */
    fun cancelRunning() {
        val kind = when (val current = dialog.value) {
            is LeaveUiState.ConfirmLeave -> if (current.busy) DepartureKind.Leave else return
            is LeaveUiState.ConfirmDelete -> if (current.busy) DepartureKind.DeleteSpace else return
            else -> return
        }
        replace(LeaveUiState.Hidden)
        cancel(kind)
    }

    // --- "Supprimer l'espace" (the only device) ----------------------------------------------------------------------------

    /** "Supprimer l'espace": true when the composable must now ask for strong biometrics, then call [deleteAuthenticated] or [authEnded]. */
    fun beginDelete(): Boolean {
        val current = dialog.value as? LeaveUiState.ConfirmDelete ?: return false
        if (!current.canConfirm) return false
        dialog.value = current.copy(authenticating = true, biometricUnavailable = false, failed = false)
        return true
    }

    /** The strong biometric check of "Supprimer l'espace" passed. */
    fun deleteAuthenticated() {
        val current = dialog.value as? LeaveUiState.ConfirmDelete ?: return
        if (!current.authenticating) return
        dialog.value = LeaveUiState.ConfirmDelete(armed = true, busy = true)
        attempt(DepartureKind.DeleteSpace) { core.deleteSpace() }
    }

    // --- "Effacer quand même" ----------------------------------------------------------------------------------------------

    /** Opens "Effacer quand même" (from the banner, or from "Supprimer ton espace ?" when the relay is too old) and checks whether it is allowed. */
    fun openErase() {
        when (val current = dialog.value) {
            LeaveUiState.Hidden -> Unit
            is LeaveUiState.ConfirmDelete -> if (current.busy || current.authenticating) return
            else -> return
        }
        val gen = replace(LeaveUiState.EraseWithoutProof(checking = true))
        arm(gen)
        checkErase(gen)
    }

    /** "Réessayer" in "Effacer quand même" after it was not allowed: asks again. */
    fun recheckErase() {
        val current = dialog.value as? LeaveUiState.EraseWithoutProof ?: return
        if (current.checking || current.busy || current.authenticating) return
        dialog.value = current.copy(checking = true, notAllowed = false, expired = false, failed = false)
        checkErase(generation)
    }

    fun setEraseChecked(checked: Boolean) {
        val current = dialog.value as? LeaveUiState.EraseWithoutProof ?: return
        if (!current.busy && !current.authenticating) dialog.value = current.copy(checked = checked)
    }

    /**
     * "Effacer ce téléphone": reads again whether the core allows it, right before the biometric prompt. True when the composable must
     * now ask for strong biometrics, then call [eraseAuthenticated] or [authEnded].
     */
    suspend fun beginErase(): Boolean {
        val current = dialog.value as? LeaveUiState.EraseWithoutProof ?: return false
        if (!current.canConfirm || core.departureRunning.value) return false
        val gen = generation
        dialog.value = current.copy(authenticating = true, biometricUnavailable = false, failed = false, expired = false)
        val allowed = try {
            core.canEraseWithoutProof()
        } catch (e: CancellationException) {
            // The screen was re-created while the core was asked: nobody will ever end this prompt.
            if (gen == generation) updateErase { it.copy(authenticating = false) }
            throw e
        }
        if (gen != generation) return false
        if (!allowed) {
            updateErase { it.copy(authenticating = false, notAllowed = true) }
            return false
        }
        return true
    }

    /** The strong biometric check of "Effacer quand même" passed. */
    fun eraseAuthenticated() {
        val current = dialog.value as? LeaveUiState.EraseWithoutProof ?: return
        if (!current.authenticating) return
        val gen = generation
        dialog.value = current.copy(authenticating = false, busy = true)
        viewModelScope.launch {
            // An attempt of the departure started meanwhile holds the core: waiting for it, with an "Annuler" that leaves (the core is not
            // asked yet, so nothing is erased). The attempt may last minutes on a bad network.
            if (core.departureRunning.value) {
                updateErase { it.copy(waiting = true) }
                core.departureRunning.first { !it }
                if (gen != generation) return@launch
                updateErase { it.copy(waiting = false) }
            }
            val result = try {
                core.eraseWithoutProof()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                EraseResult.Failed
            }
            if (gen != generation) return@launch
            when (result) {
                EraseResult.Erased -> replace(LeaveUiState.Hidden)
                // The verdict expired or was replaced while the user confirmed: nothing was erased, say so.
                EraseResult.NotAllowed -> updateErase { it.copy(busy = false, notAllowed = true, expired = true) }
                EraseResult.Failed -> updateErase { it.copy(busy = false, failed = true) }
            }
        }
    }

    /** The biometric prompt ended without success ([unavailable]: no strong biometrics enrolled). Nothing happens. */
    fun authEnded(unavailable: Boolean) {
        when (val current = dialog.value) {
            is LeaveUiState.ConfirmDelete ->
                if (current.authenticating) dialog.value = current.copy(authenticating = false, biometricUnavailable = unavailable)
            is LeaveUiState.EraseWithoutProof ->
                if (current.authenticating) dialog.value = current.copy(authenticating = false, biometricUnavailable = unavailable)
            else -> Unit
        }
    }

    // --- The banner ---------------------------------------------------------------------------------------------------------

    /**
     * "Réessayer": a gesture, the request is sent again. Off while an attempt runs. The retry of a "Leave" is sent at once (false). The retry
     * of a deletion is a new gesture that deletes the space on the relay: true when the composable must now ask for strong biometrics, then
     * call [retryAuthenticated] or [retryAuthEnded]. The automatic attempts (back-off, start-up) need no biometrics: they send the request again
     * only within 15 minutes of the gesture, and only verify after that.
     */
    fun beginRetry(): Boolean {
        val departure = core.departure.value ?: return false
        if (departure.wiping || retryPhase.value != RetryPhase.Idle || core.departureRunning.value) return false
        if (departure.kind == DepartureKind.DeleteSpace) {
            retryPhase.value = RetryPhase.Authenticating
            return true
        }
        runRetry(departure.kind)
        return false
    }

    /** The strong biometric check of the "Réessayer" of a deletion passed: the request is sent again if that deletion is still pending. */
    fun retryAuthenticated() {
        if (retryPhase.value != RetryPhase.Authenticating) return
        // The prompt took its time: the departure may have been cancelled, replaced, or an attempt may have started meanwhile.
        val departure = core.departure.value
        if (departure == null || departure.kind != DepartureKind.DeleteSpace || departure.wiping || core.departureRunning.value) {
            retryPhase.value = RetryPhase.Idle
            return
        }
        runRetry(departure.kind)
    }

    /** The biometric prompt of the "Réessayer" ended without success ([unavailable]: no strong biometrics enrolled). Nothing is sent. */
    fun retryAuthEnded(unavailable: Boolean) {
        if (retryPhase.value != RetryPhase.Authenticating) return
        retryPhase.value = RetryPhase.Idle
        if (unavailable) eventChannel.trySend(LeaveEvent.BiometricUnavailable)
    }

    private fun runRetry(kind: DepartureKind) {
        retryPhase.value = RetryPhase.Running
        viewModelScope.launch {
            try {
                val result = try {
                    core.retryDeparture()
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    DepartureResult.Failed
                }
                when {
                    result.isTransient -> followUp.kick()
                    result is DepartureResult.Refused -> if (dialog.value == LeaveUiState.Hidden) {
                        showRefusal(DepartureRefusal(kind, result.reason))
                    }
                    // Not after a cancellation: the departure is gone, the failure of the attempt that was cancelled is not news.
                    result == DepartureResult.Failed -> if (core.departure.value != null) eventChannel.trySend(LeaveEvent.Failed)
                    else -> Unit
                }
            } finally {
                retryPhase.value = RetryPhase.Idle
            }
        }
    }

    /** "Annuler" in the banner: always available but during the erasure, immediate. */
    fun cancelFromBanner() {
        val departure = core.departure.value ?: return
        if (!departure.wiping) cancel(departure.kind)
    }

    // --- Internals ----------------------------------------------------------------------------------------------------------

    private fun cancel(fallbackKind: DepartureKind) {
        val kind = core.departure.value?.kind ?: fallbackKind
        viewModelScope.launch {
            val running = core.departureRunning.value
            var cancelled = core.cancelDeparture()
            if (!cancelled && running) {
                // The attempt that runs has not written its marker yet, so there was nothing to cancel, and it would go on to erase on a
                // proof. Waits until the marker is there (or the attempt is over), then cancels again.
                combine(core.departure, core.departureRunning) { d, r -> d != null || !r }.first { it }
                if (core.departure.value != null) cancelled = core.cancelDeparture()
            }
            if (cancelled) eventChannel.trySend(LeaveEvent.Cancelled(kind))
        }
    }

    /** Runs one attempt for the open dialog and shows what it gave, unless the user closed or replaced the dialog meanwhile. */
    private fun attempt(kind: DepartureKind, call: suspend () -> DepartureResult) {
        val gen = generation
        viewModelScope.launch {
            val result = try {
                call()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                DepartureResult.Failed
            }
            if (result.isTransient) followUp.kick()
            if (gen != generation) return@launch
            when (result) {
                // Erased: the app restarts on the welcome screen. Pending: the banner takes over (and the retries).
                DepartureResult.Erased, is DepartureResult.Pending, DepartureResult.Cancelled, DepartureResult.NothingPending ->
                    replace(LeaveUiState.Hidden)
                is DepartureResult.Refused -> showRefusal(DepartureRefusal(kind, result.reason))
                DepartureResult.Failed -> when (kind) {
                    DepartureKind.Leave -> replace(LeaveUiState.ConfirmLeave(failed = true))
                    DepartureKind.DeleteSpace -> replaceArmed(LeaveUiState.ConfirmDelete(failed = true))
                }
            }
        }
    }

    /** A refusal of the relay, as a dialog: never shown as a departure that happened. */
    private fun showRefusal(refusal: DepartureRefusal) {
        when {
            // The only device: "Supprimer ton espace ?" (again, if the relay said so after a "not the last device" detour).
            refusal.reason == RefusalReason.LastDevice -> replaceArmed(LeaveUiState.ConfirmDelete())
            refusal.kind == DepartureKind.DeleteSpace -> replaceArmed(LeaveUiState.ConfirmDelete(error = refusal.reason))
            else -> replace(LeaveUiState.ConfirmLeave(error = refusal.reason))
        }
    }

    private fun checkErase(gen: Int) {
        viewModelScope.launch {
            var allowed = core.canEraseWithoutProof()
            if (!allowed) {
                // The verdict may be missing or old: ask the relay again, then read again. With a pending departure this is the
                // automatic attempt (within 15 minutes of the gesture it sends the request again, later it only verifies); without one,
                // a sync (which opens a session).
                var refusal: DepartureRefusal? = null
                try {
                    val kind = core.departure.value?.kind
                    if (kind != null) {
                        val result = core.continueDeparture()
                        if (result.isTransient) followUp.kick()
                        // "Relay too old" is what allows this very dialog: the read below says so. Any other refusal ends the departure.
                        if (result is DepartureResult.Refused && result.reason != RefusalReason.RelayTooOld) {
                            refusal = DepartureRefusal(kind, result.reason)
                        }
                    } else {
                        core.sync()
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    // Offline or refused: not allowed, which is what the read below says.
                }
                if (refusal != null) {
                    if (gen == generation) showRefusal(refusal)
                    return@launch
                }
                allowed = core.canEraseWithoutProof()
            }
            if (gen == generation) updateErase { it.copy(checking = false, notAllowed = !allowed) }
        }
    }

    private fun updateErase(change: (LeaveUiState.EraseWithoutProof) -> LeaveUiState.EraseWithoutProof) {
        val current = dialog.value as? LeaveUiState.EraseWithoutProof ?: return
        dialog.value = change(current)
    }

    /** Shows [next] in place of the current dialog; what was started for the previous one is dropped. Returns the new generation. */
    private fun replace(next: LeaveUiState): Int {
        armJob?.cancel()
        armJob = null
        dialog.value = next
        return ++generation
    }

    /** Shows a confirmation whose button is armed after [armDelayMs]. */
    private fun replaceArmed(next: LeaveUiState.ConfirmDelete) {
        val gen = replace(next.copy(armed = false))
        arm(gen)
    }

    private fun arm(gen: Int) {
        armJob?.cancel()
        armJob = viewModelScope.launch {
            delay(armDelayMs)
            if (gen != generation) return@launch
            when (val current = dialog.value) {
                is LeaveUiState.ConfirmDelete -> dialog.value = current.copy(armed = true)
                is LeaveUiState.EraseWithoutProof -> dialog.value = current.copy(armed = true)
                else -> Unit
            }
        }
    }

    companion object {
        /** The confirm button of an irreversible dialog works only this long after the dialog shows. */
        const val ARM_DELAY_MS = 2_000L
    }
}

/**
 * The dialogs of the departure. Colours and shapes come from the theme; progress is a linear bar, never a spinner. The biometric prompts
 * are asked here (they need the activity), always with strong biometrics, never remembered.
 */
@Composable
fun LeaveSpaceDialog(viewModel: LeaveSpaceViewModel) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val attemptRunning by viewModel.attemptRunning.collectAsStateWithLifecycle()
    val authenticator = rememberAuthenticator()
    val scope = rememberCoroutineScope()
    when (val current = state) {
        LeaveUiState.Hidden -> Unit
        is LeaveUiState.ConfirmLeave -> ConfirmLeaveDialog(
            state = current,
            onConfirm = viewModel::confirmLeave,
            onCancelRunning = viewModel::cancelRunning,
            onDismiss = viewModel::dismiss,
        )
        is LeaveUiState.ConfirmDelete -> ConfirmDeleteDialog(
            state = current,
            onConfirm = {
                if (viewModel.beginDelete()) {
                    scope.launch(start = CoroutineStart.UNDISPATCHED) {
                        strongPrompt(
                            prompt = { authenticator?.forSpaceDeletion() ?: AuthOutcome.Unavailable },
                            onSuccess = viewModel::deleteAuthenticated,
                            onEnded = viewModel::authEnded,
                        )
                    }
                }
            },
            onEraseAnyway = viewModel::openErase,
            onCancelRunning = viewModel::cancelRunning,
            onDismiss = viewModel::dismiss,
        )
        is LeaveUiState.EraseWithoutProof -> EraseWithoutProofDialog(
            state = current,
            attemptRunning = attemptRunning,
            onChecked = viewModel::setEraseChecked,
            onConfirm = {
                scope.launch(start = CoroutineStart.UNDISPATCHED) {
                    // The core is asked again right before the prompt (a re-creation of the screen meanwhile is handled by beginErase);
                    // after the prompt, the core decides once more (and may refuse).
                    if (!viewModel.beginErase()) return@launch
                    strongPrompt(
                        prompt = { authenticator?.forErasure() ?: AuthOutcome.Unavailable },
                        onSuccess = viewModel::eraseAuthenticated,
                        onEnded = viewModel::authEnded,
                    )
                }
            },
            onRecheck = viewModel::recheckErase,
            onDismiss = viewModel::dismiss,
        )
    }
}

/**
 * The "Réessayer" of the banner. For a deletion, the strong biometric prompt comes first, every time;
 * the retry of a plain departure is sent at once. Used by the feed, which draws the banner.
 */
@Composable
fun rememberDepartureRetry(viewModel: LeaveSpaceViewModel): () -> Unit {
    val authenticator = rememberAuthenticator()
    val scope = rememberCoroutineScope()
    return {
        if (viewModel.beginRetry()) {
            scope.launch(start = CoroutineStart.UNDISPATCHED) {
                strongPrompt(
                    prompt = { authenticator?.forSpaceDeletion() ?: AuthOutcome.Unavailable },
                    onSuccess = viewModel::retryAuthenticated,
                    onEnded = viewModel::retryAuthEnded,
                )
            }
        }
    }
}

/**
 * Asks for strong biometrics and tells the view model how it ended. If the scope goes away while the prompt is up (the activity is
 * re-created, a rotation for instance), the prompt counts as cancelled: the view model never stays locked on a prompt nobody listens to.
 */
internal suspend fun strongPrompt(
    prompt: suspend () -> AuthOutcome,
    onSuccess: () -> Unit,
    onEnded: (unavailable: Boolean) -> Unit,
) {
    var settled = false
    try {
        // No activity able to show a prompt: the caller answers Unavailable, the action stays locked.
        when (prompt()) {
            AuthOutcome.Success -> {
                settled = true
                onSuccess()
            }
            AuthOutcome.Unavailable -> {
                settled = true
                onEnded(true)
            }
            AuthOutcome.Cancelled, AuthOutcome.Failed -> {
                settled = true
                onEnded(false)
            }
        }
    } finally {
        if (!settled) onEnded(false)
    }
}

@Composable
private fun ConfirmLeaveDialog(
    state: LeaveUiState.ConfirmLeave,
    onConfirm: () -> Unit,
    onCancelRunning: () -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(dismissOnBackPress = !state.busy, dismissOnClickOutside = !state.busy),
        title = { Text(stringResource(R.string.leave_title)) },
        text = {
            DialogBody(
                messages = listOf(stringResource(R.string.leave_message)),
                busyText = stringResource(R.string.leave_in_progress),
                busy = state.busy,
                error = when {
                    state.failed -> stringResource(R.string.departure_failed)
                    state.error == RefusalReason.UpdateRequired -> stringResource(R.string.leave_error_update)
                    state.error != null -> stringResource(R.string.leave_error_unexpected)
                    else -> null
                },
            )
        },
        confirmButton = {
            if (!state.busy) DangerButton(stringResource(R.string.leave_confirm), enabled = true, onClick = onConfirm)
        },
        dismissButton = {
            if (state.busy) {
                // Always available while the attempt runs (it may last minutes on a bad network): cancels at once, erases nothing.
                DialogTextButton(stringResource(R.string.departure_cancel), onCancelRunning)
            } else {
                DialogTextButton(stringResource(R.string.action_cancel), onDismiss)
            }
        },
    )
}

@Composable
private fun ConfirmDeleteDialog(
    state: LeaveUiState.ConfirmDelete,
    onConfirm: () -> Unit,
    onEraseAnyway: () -> Unit,
    onCancelRunning: () -> Unit,
    onDismiss: () -> Unit,
) {
    val tooOld = state.error == RefusalReason.RelayTooOld
    val locked = state.busy || state.authenticating
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(dismissOnBackPress = !locked, dismissOnClickOutside = !locked),
        title = { Text(stringResource(R.string.delete_space_title)) },
        text = {
            DialogBody(
                messages = listOf(stringResource(R.string.delete_space_message), stringResource(R.string.delete_space_detail)),
                busyText = stringResource(R.string.delete_space_in_progress),
                busy = state.busy,
                error = when {
                    state.biometricUnavailable -> stringResource(R.string.delete_space_unavailable_biometric)
                    state.failed -> stringResource(R.string.departure_failed)
                    tooOld -> stringResource(R.string.delete_space_error_relay_too_old)
                    state.error == RefusalReason.UpdateRequired -> stringResource(R.string.delete_space_error_update)
                    state.error == RefusalReason.Unexpected -> stringResource(R.string.delete_space_error_unexpected)
                    else -> null
                },
            )
        },
        confirmButton = {
            when {
                state.busy -> Unit
                tooOld -> DangerButton(stringResource(R.string.erase_anyway), enabled = !locked, onClick = onEraseAnyway)
                else -> DangerButton(stringResource(R.string.delete_space_confirm), enabled = state.canConfirm, onClick = onConfirm)
            }
        },
        dismissButton = {
            if (state.busy) {
                DialogTextButton(stringResource(R.string.departure_cancel), onCancelRunning)
            } else {
                DialogTextButton(stringResource(R.string.action_cancel), onDismiss, enabled = !state.authenticating)
            }
        },
    )
}

/** The texts of a dialog, then the error of the last try (announced to screen readers) or, while the call runs, a linear progress bar. */
@Composable
internal fun DialogBody(
    messages: List<String>,
    busyText: String,
    busy: Boolean,
    error: String?,
    /** Scrolls by default (large fonts, small screens, landscape); a caller that scrolls a bigger column around this one passes its own. */
    modifier: Modifier = Modifier.verticalScroll(rememberScrollState()),
) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        messages.forEach { Text(it, style = MaterialTheme.typography.bodyMedium) }
        if (busy) {
            Text(busyText, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
            LinearProgressIndicator(Modifier.fillMaxWidth())
        } else if (error != null) {
            Text(
                error,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )
        }
    }
}

/** A text button in the error colour (the destructive action of a dialog), 48 dp high. */
@Composable
internal fun DangerButton(text: String, enabled: Boolean, onClick: () -> Unit) {
    TextButton(
        onClick = onClick,
        enabled = enabled,
        colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
        modifier = Modifier.heightIn(min = 48.dp),
    ) { Text(text) }
}

@Composable
internal fun DialogTextButton(text: String, onClick: () -> Unit, enabled: Boolean = true) {
    TextButton(onClick = onClick, enabled = enabled, modifier = Modifier.heightIn(min = 48.dp)) { Text(text) }
}
