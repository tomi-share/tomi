package eu.shaffe.tomi.ui.pairing

import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.PairingException
import eu.shaffe.tomi.core.PairingFailure
import eu.shaffe.tomi.core.PairingResult
import eu.shaffe.tomi.core.LinkReader
import eu.shaffe.tomi.core.RelayAddress
import eu.shaffe.tomi.core.RelayChoice
import eu.shaffe.tomi.core.TomiCoreException
import eu.shaffe.tomi.deeplink.DeepLink
import eu.shaffe.tomi.deeplink.DeepLinkParser
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/** Input (scan / code) -> Working -> Verify (6-digit code) -> History (one yes/no) -> Waiting (the other person) -> Done. */
enum class PairingPhase { Input, Working, Verify, History, Waiting, Done }

object PairingInput {
    const val MAX_CODE_INPUT = 64

    /**
     * A typed code such as `7-crayon-renard`. The core reads it (spec/pairing-v1.md 4.4: separators, capitals, accents, the first four
     * letters of a word), and a code it cannot read costs nothing: only what is clearly not a code is refused here.
     */
    fun isValidCode(raw: String): Boolean {
        val code = raw.trim()
        return code.isNotEmpty() && code.length <= MAX_CODE_INPUT && code.none { it.isISOControl() }
    }

    /**
     * A scanned QR must be a pairing link (`https://<relay>/l/pair#<code>` for any relay, or `tomi-share://pair#<code>`), nothing else. The relay
     * it names is not trusted: it is shown in full on the verification screen, and the core keeps it only once the pairing succeeded.
     */
    fun linkFromQr(raw: String, links: LinkReader): DeepLink.Pair? = DeepLinkParser.parse(raw.trim(), links) as? DeepLink.Pair
}

/** The message of a failure of the new device's pairing. */
internal fun joinFailureMessage(reason: PairingFailure): Int = when (reason) {
    PairingFailure.InvalidCode -> R.string.pair_error_invalid
    PairingFailure.WrongCode -> R.string.pair_error_wrong_code
    PairingFailure.Expired -> R.string.pair_error_expired
    PairingFailure.Network -> R.string.pair_error_network
    PairingFailure.Taken -> R.string.pair_error_taken
    PairingFailure.Interrupted -> R.string.pair_error_interrupted
    PairingFailure.Cancelled -> R.string.pair_error_cancelled
    PairingFailure.UpdateRequired -> R.string.pair_error_update
    PairingFailure.Blocked -> R.string.pair_error_blocked
    PairingFailure.SpaceFull -> R.string.pair_error_space_full
    PairingFailure.RelayNotConfigured -> R.string.pair_error_relay_required
    PairingFailure.InvalidRelay -> R.string.pair_error_relay_invalid
    PairingFailure.AnotherPairingOpen, PairingFailure.InstallTokenInvalid, PairingFailure.SpaceExists, PairingFailure.Unexpected ->
        R.string.pair_error_unexpected
}

class PairingViewModel(private val container: AppContainer) : ViewModel() {
    var phase by mutableStateOf(PairingPhase.Input)
        private set
    var error by mutableStateOf<Int?>(null)
        private set
    var code by mutableStateOf("")
        private set

    /**
     * Set while the user compares the 6-digit codes: the other device was reached, nothing is joined yet. The verification code inside is a
     * [eu.shaffe.tomi.core.VerificationCode]: it prints as redacted, and only the secure window of this screen draws it.
     */
    var awaiting by mutableStateOf<PairingResult.AwaitingVerification?>(null)
        private set

    /** `SystemClock.elapsedRealtime()` at which the relay drops the channel, by the estimate of the core (indicative, see [PairingResult]). */
    var validUntilElapsedMs by mutableLongStateOf(0L)
        private set

    /** The single yes/no answer of the history step. "Yes" is preselected. */
    var importHistory by mutableStateOf(true)
        private set

    /** True while the final confirmation is being sent to the core. */
    var busy by mutableStateOf(false)
        private set

    /** Payload received through an App Link or a QR code. It is only offered to the user, never used automatically. */
    var pendingPayload by mutableStateOf<String?>(null)
        private set

    /** The relay named by the pending link (`https://host[:port]`), not trusted: shown on the card, again on the verification screen. */
    var pendingRelay by mutableStateOf<String?>(null)
        private set

    /** The relay typed in the "Relais" field, as written. */
    var relayText by mutableStateOf("")
        private set

    /** The relay of the device (`TomiCore.relayUrl`), null in a release build that has none yet; read once at start. */
    var configuredRelay by mutableStateOf<String?>(null)
        private set

    /** The field is open and required: no relay on the device and none in the link. */
    val relayRequired: Boolean get() = RelayAddress.fieldRequired(configuredRelay, pendingRelay)

    /** Incremented at each start and each abort: the result of an attempt that was abandoned meanwhile is dropped. */
    private var attempt = 0
    private var job: Job? = null

    init {
        var wasPaired = container.core.isPaired.value
        viewModelScope.launch {
            try {
                configuredRelay = container.core.relayUrl()
            } catch (e: CancellationException) {
                throw e
            } catch (_: TomiCoreException) {
                // No relay known: the field stays required.
            }
        }
        viewModelScope.launch {
            container.core.isPaired.collect { paired ->
                // A pairing that a stop interrupted is settled at start-up: the device is in the space while this screen waits.
                if (paired && !wasPaired && phase == PairingPhase.Input) {
                    clearPending()
                    phase = PairingPhase.Done
                }
                wasPaired = paired
            }
        }
    }

    fun onCodeChange(value: String) {
        code = value.take(PairingInput.MAX_CODE_INPUT)
        error = null
    }

    fun onRelayChange(value: String) {
        relayText = value.take(RelayAddress.MAX_INPUT)
        error = null
    }

    fun offerPayload(payload: String, relayUrl: String? = null) {
        pendingPayload = payload
        pendingRelay = relayUrl
    }

    fun joinWithPendingPayload() {
        pendingPayload?.let(::join)
    }

    /** The pending link is dropped (it was used, or it cannot work). */
    private fun clearPending() {
        pendingPayload = null
        pendingRelay = null
    }

    fun onQrScanned(raw: String) {
        val link = PairingInput.linkFromQr(raw, container.links)
        if (link == null) {
            error = R.string.pair_error_qr_unknown
            return
        }
        pendingRelay = link.relayUrl
        join(link.payload)
    }

    fun joinWithCode() {
        if (!PairingInput.isValidCode(code)) {
            error = R.string.pair_error_code_format
            return
        }
        // Nothing is sent before the relay is settled: a typed relay is checked by the core's own reading of addresses.
        val relay = when (val choice = RelayAddress.choose(relayText, configuredRelay, container.links)) {
            RelayChoice.DeviceRelay -> null
            is RelayChoice.Typed -> choice.url
            RelayChoice.Invalid -> {
                error = R.string.pair_error_relay_invalid
                return
            }
            RelayChoice.Missing -> {
                error = R.string.pair_error_relay_required
                return
            }
        }
        start { container.core.pairWithCode(code.trim(), relay) }
    }

    fun onImportHistoryChange(value: Boolean) {
        importHistory = value
    }

    /** "The codes are identical": ask about the history if the other device offers any, otherwise finish. */
    fun codesMatch() {
        val pending = awaiting ?: return
        if (pending.historyItemCount > 0) phase = PairingPhase.History else finish(importHistory = false)
    }

    /** "They are different": the pairing is cancelled, nothing is joined. */
    fun codesDiffer() {
        abort(R.string.pair_verify_mismatch)
    }

    /** Back or "Annuler" at any step: same as a refusal, without the error message. */
    fun leave() {
        abort(null)
    }

    fun confirmHistory() = finish(importHistory)

    private fun abort(errorRes: Int?) {
        attempt++
        job?.cancel()
        job = null
        awaiting = null
        busy = false
        phase = PairingPhase.Input
        error = errorRes
        closePendingPairing()
    }

    /** Closes the pairing at the relay and wipes its secrets, whatever state the core is in. Idempotent. */
    private fun closePendingPairing() {
        container.scope.launch {
            try {
                container.core.cancelPairing()
            } catch (e: CancellationException) {
                throw e
            } catch (_: TomiCoreException) {
                // Nothing was joined: there is nothing more to undo.
            }
        }
    }

    private fun finish(importHistory: Boolean) {
        if (busy) return
        busy = true
        phase = PairingPhase.Waiting
        val mine = ++attempt
        job = viewModelScope.launch {
            try {
                container.core.confirmPairing(importHistory)
                if (mine == attempt) {
                    awaiting = null
                    // A confirmation without a pending pairing does nothing and returns: success is the device being in the space.
                    if (container.core.isPaired.value) {
                        phase = PairingPhase.Done
                    } else {
                        phase = PairingPhase.Input
                        error = R.string.pair_error_unexpected
                    }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: TomiCoreException) {
                if (mine == attempt) {
                    awaiting = null
                    phase = PairingPhase.Input
                    error = joinFailureMessage((e as? PairingException)?.reason ?: PairingFailure.Unexpected)
                    // The session may still be alive (a transient error): do not leave it open at the relay.
                    closePendingPairing()
                }
            } finally {
                if (mine == attempt) busy = false
            }
        }
    }

    private fun join(payload: String) = start { container.core.pairWithQr(payload) }

    private fun start(block: suspend () -> PairingResult) {
        if (phase == PairingPhase.Working) return
        phase = PairingPhase.Working
        error = null
        val mine = ++attempt
        job = viewModelScope.launch {
            val result = try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (_: TomiCoreException) {
                PairingResult.Failure(PairingFailure.Unexpected)
            }
            // Abandoned while it ran ("Annuler", back): the core was told, and this answer is no longer wanted.
            if (mine != attempt) return@launch
            when (result) {
                is PairingResult.AwaitingVerification -> {
                    // The code was used: it is a one-time secret, nothing keeps it.
                    clearPending()
                    code = ""
                    awaiting = result
                    validUntilElapsedMs = SystemClock.elapsedRealtime() + result.expiresInMs
                    importHistory = true
                    phase = PairingPhase.Verify
                }
                is PairingResult.Failure -> {
                    phase = PairingPhase.Input
                    error = joinFailureMessage(result.reason)
                    // A link that cannot work is not offered again (unless the network was the problem). A typed code that was refused for
                    // good is gone from the screen; one that cannot be read (a typo) or that never reached the relay stays, to be
                    // fixed or tried again.
                    if (result.reason != PairingFailure.Network) clearPending()
                    if (result.reason != PairingFailure.InvalidCode && result.reason != PairingFailure.Network) code = ""
                }
                PairingResult.AlreadyPaired -> {
                    clearPending()
                    code = ""
                    phase = PairingPhase.Done
                }
            }
        }
    }
}
