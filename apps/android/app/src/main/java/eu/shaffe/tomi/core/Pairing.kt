package eu.shaffe.tomi.core

/*
 * Pairing, as the UI sees it (spec/pairing-v1.md). Three roles:
 * - the new device ("N") joins a space: pairWithCode / pairWithQr, confirmPairing, cancelPairing (TomiCore);
 * - an existing device ("A") adds one: TomiCore.startInvitation -> [InviteSession];
 * - the first device creates the space: TomiCore.createSpace.
 *
 * Everything that is secret on the way is wrapped in a type whose toString() is redacted: the verification code ([VerificationCode]),
 * the pairing code and its link ([SensitiveText]). Nothing here carries a key: the core does the whole exchange and the keys never cross
 * the boundary.
 */

/** Why a pairing step (joining, inviting, creating the space) did not go through. Each value has its own message in the UI. */
enum class PairingFailure {
    /** The code or the link cannot be read. Nothing was consumed: the user can fix the typo and retry. */
    InvalidCode,

    /** `pairing_failed`: the key exchange failed. Deliberately indistinguishable from a wrong code (spec 11.1): get a new code. */
    WrongCode,

    /** `pairing_expired`: the code ran out. */
    Expired,

    /** The relay could not be reached, or answered with a temporary error. Try again. */
    Network,

    /** `pairing_taken`: another device already used this code. */
    Taken,

    /**
     * The connection broke after this device used the code: the code is spent (one use), so retrying is pointless; the other device makes a
     * new one. Only the new device ([TomiCore.pairWithCode], [TomiCore.confirmPairing]) reports it; before the code is used a broken
     * connection is a plain [Network].
     */
    Interrupted,

    /** `pairing_cancelled`: the other device (or the user) cancelled. */
    Cancelled,

    /** `pairing_unsupported`, or a relay that requires a newer app: update Tomi. */
    UpdateRequired,

    /** The space already has the maximum number of devices (`too_many_devices`). */
    SpaceFull,

    /** Two pairings are already open at the relay (`channel_limit`). */
    AnotherPairingOpen,

    /** `install_token_invalid`: wrong installation token. The token was not consumed. */
    InstallTokenInvalid,

    /** `space_exists`: another device already created the space. The user should join it instead. */
    SpaceExists,

    /** This device can no longer use the relay (removed from the space...). */
    Blocked,

    /** `relay_not_configured`: the build has no relay and neither a link nor the "Relais" field gave one. Nothing was sent. */
    RelayNotConfigured,

    /** The relay that was typed or read is not a valid relay address (https, host, optional port). Nothing was sent. */
    InvalidRelay,

    /** Storage failure, internal error or a refusal the app has no message for. */
    Unexpected,
}

/** Error of the pairing calls that cannot be a [PairingResult] (confirm, invitation, space creation). */
class PairingException(val reason: PairingFailure, cause: Throwable? = null) :
    TomiCoreException("pairing failed: ${reason.name}", cause)

/** The other device, as it announced itself on the authenticated channel. */
data class PairingPeer(val name: String, val platform: DevicePlatform)

/**
 * The 6-digit verification code, compared by eye on both devices (spec 5.3). It exists to be drawn on a secure window and nothing else:
 * never logged, never typed, never sent, never in a notification. [toString] is redacted so that a log line, a crash report or a
 * `"$state"` cannot leak it; [digits] is the only way out and its callers draw it.
 *
 * The core hands it over as `"305 718"`; only the digits are kept, whatever the separator.
 */
@JvmInline
value class VerificationCode(private val raw: String) {
    /** The digits, without separator (`"305718"`), for the digit boxes. */
    val digits: String get() = raw.filter { it.isDigit() }

    /** The digits in two groups of three (`"305 718"`), for the screen reader. */
    val grouped: String get() = digits.chunked(GROUP).joinToString(" ")

    override fun toString(): String = "VerificationCode(<redacted>)"

    private companion object {
        const val GROUP = 3
    }
}

/**
 * A secret string that is only meant to be shown: the pairing code (`4821-crayon-renard`), its link (the QR code) or an installation
 * token. Never logged, copied to the clipboard, stored or sent. [toString] is redacted; [reveal] is for the composable that draws it.
 */
@JvmInline
value class SensitiveText(private val value: String) {
    fun reveal(): String = value

    val isBlank: Boolean get() = value.isBlank()

    override fun toString(): String = "SensitiveText(<redacted>)"
}

sealed interface PairingResult {
    /**
     * The other device was reached but this device has NOT joined the space yet: the user must compare [verificationCode] with the one
     * shown on the other device, then the app calls [TomiCore.confirmPairing] (or [TomiCore.cancelPairing] when the codes differ).
     *
     * [historyItemCount] is the number of items the other device offers to transfer (0 = nothing to ask: the question is not asked);
     * [historyBytes] is their approximate size. Both are estimates, for display only. [memberCount] is the number of devices of the
     * space, the other one included, when it told. [expiresInMs] is the time left before the relay drops the channel: indicative on
     * this side (the clock of this device is not synchronised with the relay yet), use it for a rough countdown, never as a cut-off.
     *
     * [relayAuthority] is the relay of this pairing (`host[:port]`, `relay.example.org`). It may have been read from a QR code, a link or a
     * field, so it is NOT trusted: the screen shows it in full next to the verification code and the user confirms it with the code
     * (spec/pairing-v1.md 4.4). Nothing is kept before that confirmation.
     */
    data class AwaitingVerification(
        val peer: PairingPeer,
        val verificationCode: VerificationCode,
        val historyItemCount: Int,
        val historyBytes: Long = 0L,
        val memberCount: Int? = null,
        val expiresInMs: Long = 0L,
        val relayAuthority: String = "",
    ) : PairingResult

    data class Failure(val reason: PairingFailure) : PairingResult

    /** This device already belongs to a space (a pairing that a stop interrupted was settled at start-up): nothing to join. */
    data object AlreadyPaired : PairingResult
}

/** What [TomiCore.resumePairing] found. */
enum class ResumeOutcome {
    /** Nothing was pending. */
    NothingPending,

    /** The pairing or the space creation that a stop interrupted is complete: this device now belongs to the space. */
    Provisioned,

    /** The relay did not register this device: the pending batch was erased, the user starts again. */
    Discarded,

    /** The relay could not be reached after the retries: it is settled at the next start (or the next call). */
    Unreachable,
}

/** What the user of the inviting device compares and decides on. */
data class InvitePrompt(val peer: PairingPeer, val verificationCode: VerificationCode)

/** The new device, registered in the space. [historyAnnounced]: it asked for the history, the export starts at the next [TomiCore.sync]. */
data class InvitedDevice(val deviceId: String, val name: String, val platform: DevicePlatform, val historyAnnounced: Boolean)

/**
 * One invitation, from the code to the registered device: this device is a member and adds a new one ("A"). Obtained from
 * [TomiCore.startInvitation]. The core drives nothing by itself: the suspending calls below ARE the driver.
 *
 * ```
 * code() / link() on screen -> awaitPeer() -> verification code + new device on screen
 *   -> [user: "Add"] confirm() -> awaitDone()                 [user: "Cancel" or leaves the screen: cancel()]
 * ```
 *
 * The suspending calls wait for the network and for the other person (minutes at most). Cancelling the coroutine that waits ends the
 * whole invitation, like [cancel]. Failures are [PairingException].
 */
interface InviteSession {
    /** Identifier of the channel at the relay (not secret). */
    val channelId: String

    /** The code to show (`4821-crayon-renard`), in a large monospace font; null once the session is over. Never copied anywhere. */
    fun code(): SensitiveText?

    /** The link of the QR code, to draw exactly as it is; null once the session is over. */
    fun link(): SensitiveText?

    /** Milliseconds left before the relay drops the channel if nobody used the code (never negative). At 0, start a new invitation. */
    fun expiresInMs(): Long

    /** Suspends until the new device said hello: its name and the verification code to compare. [PairingFailure.Expired] when the code ran out. */
    suspend fun awaitPeer(): InvitePrompt

    /**
     * The user compared the codes and pressed "Add". This alone does not give the new device anything: the space key leaves this device
     * only once the user of the new device confirmed as well. Follow with [awaitDone].
     */
    suspend fun confirm()

    /** Suspends until the new device is part of the space. Call [TomiCore.sync] afterwards: it starts the export of the history. */
    suspend fun awaitDone(): InvitedDevice

    /** The user refused or left the screen. Closes the channel at the relay and wipes the session. Safe to call at any time, more than once. */
    suspend fun cancel()
}
