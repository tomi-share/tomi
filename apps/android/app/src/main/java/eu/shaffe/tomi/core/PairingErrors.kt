package eu.shaffe.tomi.core

import uniffi.tomi_core.TomiException

/**
 * What the Rust core throws, as the UI understands it (crates/core/src/ffi.rs, `TomiError`; spec/pairing-v1.md 11.5).
 *
 * The messages of these exceptions never carry the code, the verification code, a key or a frame (spec 11.5, 12): the original exception
 * is kept as the cause of what is thrown here.
 */
internal object PairingErrors {
    /** The stable `Crypto { code }` values of the pairing (spec 11.5). */
    private const val CODE_FAILED = "pairing_failed"
    private const val CODE_EXPIRED = "pairing_expired"
    private const val CODE_TAKEN = "pairing_taken"
    private const val CODE_CANCELLED = "pairing_cancelled"
    private const val CODE_UNSUPPORTED = "pairing_unsupported"
    private const val CODE_RELAY_NOT_CONFIGURED = "relay_not_configured"

    private const val STATUS_RATE_LIMITED = 429
    private const val STATUS_SERVER_ERROR = 500

    val DEFAULT_RETRY_DELAYS_MS: List<Long> = listOf(1_000L, 2_000L, 4_000L)

    fun failureOf(e: TomiException): PairingFailure = when (e) {
        is TomiException.Crypto -> when (e.code) {
            CODE_FAILED -> PairingFailure.WrongCode
            CODE_EXPIRED -> PairingFailure.Expired
            CODE_TAKEN -> PairingFailure.Taken
            CODE_CANCELLED -> PairingFailure.Cancelled
            CODE_UNSUPPORTED -> PairingFailure.UpdateRequired
            CODE_RELAY_NOT_CONFIGURED -> PairingFailure.RelayNotConfigured
            else -> PairingFailure.Unexpected
        }
        is TomiException.Network -> PairingFailure.Network
        is TomiException.Relay -> relayFailure(e.status.toInt(), e.code)
        // The reason is the Debug form of the core's `Blocked` ("UpdateRequired { .. }", "Revoked", "NotFound"...).
        is TomiException.Blocked -> if (e.reason.startsWith("UpdateRequired")) PairingFailure.UpdateRequired else PairingFailure.Blocked
        // After the checks of RealTomiCore.join (paired? valid name?) the only InvalidInput left of join_space is an unreadable code:
        // the caller maps it itself. Anywhere else it is an unexpected state ("not the moment", already provisioned...).
        is TomiException.InvalidInput, is TomiException.Storage, is TomiException.Internal -> PairingFailure.Unexpected
    }

    private fun relayFailure(status: Int, code: String): PairingFailure = when {
        status == STATUS_RATE_LIMITED || status >= STATUS_SERVER_ERROR -> PairingFailure.Network
        code == "too_many_devices" -> PairingFailure.SpaceFull
        code == "channel_limit" -> PairingFailure.AnotherPairingOpen
        code == "install_token_invalid" -> PairingFailure.InstallTokenInvalid
        code == "space_exists" -> PairingFailure.SpaceExists
        code == "update_required" || code == "format_unsupported" -> PairingFailure.UpdateRequired
        else -> PairingFailure.Unexpected
    }

    /**
     * The errors after which a session is left as it was ("call again"): `Network`, `Storage`, and `Relay` 429 or 5xx
     * (crates/core/src/ffi.rs, module documentation).
     */
    fun isTransient(e: TomiException): Boolean = when (e) {
        is TomiException.Network, is TomiException.Storage -> true
        is TomiException.Relay -> e.status.toInt().let { it == STATUS_RATE_LIMITED || it >= STATUS_SERVER_ERROR }
        else -> false
    }

    /**
     * Runs a blocking call of a pairing session and calls it again after a transient error, a few times, waiting 1 s, 2 s, 4 s on this
     * (background) thread. The session is alive after such an error; a phone that changes from Wi-Fi to mobile data in the middle of a long
     * poll must not cost the user a new code. Any other error, or the last transient one, is thrown.
     */
    fun <T> retryingTransient(
        delaysMs: List<Long> = DEFAULT_RETRY_DELAYS_MS,
        sleep: (Long) -> Unit = { Thread.sleep(it) },
        block: () -> T,
    ): T {
        var attempt = 0
        while (true) {
            try {
                return block()
            } catch (e: TomiException) {
                if (!isTransient(e) || attempt >= delaysMs.size) throw e
                sleep(delaysMs[attempt++])
            }
        }
    }

    /** What a non-pairing call throws: [TomiCoreException], with the core's exception as its cause. */
    fun coreException(e: TomiException): TomiCoreException = TomiCoreException(
        when (e) {
            is TomiException.Storage -> "core: storage failure"
            is TomiException.Network -> "core: relay unreachable"
            is TomiException.Relay -> "core: relay refused (${e.status} ${e.code})"
            is TomiException.Blocked -> "core: device blocked"
            is TomiException.InvalidInput -> "core: invalid input"
            is TomiException.Crypto -> "core: ${e.code}"
            is TomiException.Internal -> "core: internal error"
        },
        e,
    )
}
