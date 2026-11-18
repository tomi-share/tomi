package eu.shaffe.tomi.share

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.send.CancelOutcome
import eu.shaffe.tomi.send.Submission
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

sealed interface ShareUiState {
    data object Loading : ShareUiState

    /** This device has no space yet: nothing can be sent. */
    data object NotPaired : ShareUiState

    /** The undo came too late: the item already left, nothing was deleted. */
    data object TooLate : ShareUiState

    /** The intent carried nothing usable. */
    data object NothingToShare : ShareUiState

    /** Confirmation for content that looks like a secret (instead of the immediate send). */
    data class Confirm(val parsed: ParsedShare, val secret: Boolean, val secretLocked: Boolean, val expiry: ExpiryChoice) : ShareUiState

    /** Queued; the item really leaves after the undo window. */
    data class Sent(val submission: Submission) : ShareUiState

    /** Options sheet opened from the "Sent" banner. The previous send has been cancelled. */
    data class Options(val parsed: ParsedShare, val secret: Boolean, val expiry: ExpiryChoice) : ShareUiState

    data object Failed : ShareUiState
}

class ShareViewModel(private val container: AppContainer) : ViewModel() {
    private val mutableState = MutableStateFlow<ShareUiState>(ShareUiState.Loading)
    val state: StateFlow<ShareUiState> = mutableState.asStateFlow()

    private var started = false

    /** True once [start] ran: lets the activity detect a process-death restore, which must not send again. */
    val hasStarted: Boolean get() = started

    /**
     * [loadInput] gathers the share, including the content provider queries: it is run in the view model's scope,
     * and is expected to move blocking work off the main thread (see [ShareIntentReader.resolve]).
     */
    fun start(loadInput: suspend () -> ShareInput) {
        if (started) return
        started = true
        if (!container.core.isPaired.value) {
            mutableState.value = ShareUiState.NotPaired
            return
        }
        viewModelScope.launch {
            val input = try {
                loadInput()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                mutableState.value = ShareUiState.Failed
                return@launch
            }
            val parsed = SharePayloadParser.parse(input, container.ownAuthorities)
            when {
                parsed.isEmpty -> mutableState.value = ShareUiState.NothingToShare
                // Secret-looking content never goes out on a single tap: ask first.
                parsed.looksSecret -> mutableState.value = ShareUiState.Confirm(
                    parsed = parsed,
                    secret = true,
                    secretLocked = input.markedSensitive,
                    expiry = ExpiryPolicy.DEFAULT,
                )
                // Immediate send with the default settings; the banner offers Undo / Options.
                else -> submit(parsed, secret = false, expiry = ExpiryPolicy.DEFAULT)
            }
        }
    }

    fun onSecretChange(value: Boolean) {
        when (val current = mutableState.value) {
            is ShareUiState.Confirm -> if (!current.secretLocked || value) {
                mutableState.value = current.copy(
                    secret = value,
                    expiry = ExpiryPolicy.onContentChanged(current.expiry, value, current.parsed.streams.isNotEmpty()),
                )
            }
            is ShareUiState.Options ->
                mutableState.value = current.copy(
                    secret = value,
                    expiry = ExpiryPolicy.onContentChanged(current.expiry, value, current.parsed.streams.isNotEmpty()),
                )
            else -> Unit
        }
    }

    fun onExpiryChange(choice: ExpiryChoice) {
        when (val current = mutableState.value) {
            is ShareUiState.Confirm ->
                if (ExpiryPolicy.isAllowed(choice, current.secret, current.parsed.streams.isNotEmpty())) mutableState.value = current.copy(expiry = choice)
            is ShareUiState.Options ->
                if (ExpiryPolicy.isAllowed(choice, current.secret, current.parsed.streams.isNotEmpty())) mutableState.value = current.copy(expiry = choice)
            else -> Unit
        }
    }

    /** "Send" in the confirmation or options sheet. */
    fun confirm() {
        when (val current = mutableState.value) {
            is ShareUiState.Confirm -> submit(current.parsed, current.secret, current.expiry)
            is ShareUiState.Options -> submit(current.parsed, current.secret, current.expiry)
            else -> Unit
        }
    }

    /** Undo from the banner, or cancel from a sheet. Returns through [onDone] when the activity can close. */
    fun undo(onDone: () -> Unit) {
        val current = mutableState.value
        viewModelScope.launch {
            if (current is ShareUiState.Sent && !cancelSucceeded(current.submission)) return@launch
            onDone()
        }
    }

    /** "Options" in the banner: cancel the queued send and edit its settings. */
    fun openOptions() {
        val current = mutableState.value as? ShareUiState.Sent ?: return
        viewModelScope.launch {
            if (!cancelSucceeded(current.submission)) return@launch
            val sub = current.submission
            mutableState.value = ShareUiState.Options(sub.parsed, sub.secret, sub.expiry)
        }
    }

    /** Cancels the send; if the item already left (or the core failed) the state becomes [ShareUiState.TooLate] / [ShareUiState.Failed]. */
    private suspend fun cancelSucceeded(submission: Submission): Boolean {
        val outcome = try {
            container.sendCoordinator.cancel(submission)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            mutableState.value = ShareUiState.Failed
            return false
        }
        if (outcome == CancelOutcome.TooLate) {
            mutableState.value = ShareUiState.TooLate
            return false
        }
        return true
    }

    private fun submit(parsed: ParsedShare, secret: Boolean, expiry: ExpiryChoice) {
        viewModelScope.launch {
            mutableState.value = try {
                ShareUiState.Sent(container.sendCoordinator.submit(parsed, secret, expiry))
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // Any failure of staging or of the core (TomiCoreException, I/O, ...) ends in the "failed" card, never a crash.
                ShareUiState.Failed
            }
        }
    }
}
