package eu.shaffe.tomi.core

/*
 * Leaving the space and deleting it, as the app sees it (spec/api-v1.md 7.9).
 *
 * The core erases this device only on a positive proof of the relay (a `200` with `seq` to `DELETE /v1/me`, `403 revoked`, or
 * `200 {"space_deleted": true}`). Anything else leaves a pending departure that the app shows at all times ([TomiCore.departure]), with
 * "Réessayer" and "Annuler". The one way to erase without proof is "Effacer quand même" ([TomiCore.eraseWithoutProof]), behind strong
 * biometrics, and only while the core allows it.
 */

/** Which departure is pending. */
enum class DepartureKind {
    /** "Se déconnecter" (`DELETE /v1/me`). */
    Leave,

    /** "Supprimer l'espace", the last device (`DELETE /v1/space`). */
    DeleteSpace,
}

/** Why a departure is still pending. Nothing was erased. */
enum class PendingReason {
    /** Network error, timeout, unexpected answer. Retried. */
    Network,

    /** `5xx`, `429`. Retried. */
    Server,

    /** The relay does not find the space or this device ("espace introuvable"). Not a proof of anything: "Effacer quand même" may be offered. */
    NotFound,

    /** Verification only, more than 15 minutes after the gesture: the device is still a member, the request had no effect. */
    NotSent,
}

/** Why the relay refused a departure for good. Nothing was erased and nothing is pending any more. */
enum class RefusalReason {
    /** The only device of the space: the app offers "Supprimer l'espace". */
    LastDevice,

    /** This app is too old for the space. */
    UpdateRequired,

    /** The relay cannot delete a space (an older version): "Effacer quand même" is allowed. */
    RelayTooOld,

    /** Any other refusal of the relay. */
    Unexpected,
}

/** What one attempt of a departure gave. */
sealed interface DepartureResult {
    /** The relay proved it and this device holds nothing of the space any more ([TomiCore.isPaired] is false). */
    data object Erased : DepartureResult

    /** Nothing was erased; the departure stays pending, visible and cancellable. */
    data class Pending(val reason: PendingReason) : DepartureResult

    /** Nothing was erased; the departure is no longer pending. */
    data class Refused(val reason: RefusalReason) : DepartureResult

    /** Cancelled meanwhile: nothing was erased. */
    data object Cancelled : DepartureResult

    /** No departure pending ([TomiCore.continueDeparture]). Not named `Nothing`: that is a Kotlin built-in type. */
    data object NothingPending : DepartureResult

    /** The storage refused the erasure, or the call failed unexpectedly: nothing was erased (a pending departure stays pending). */
    data object Failed : DepartureResult
}

/** Pending on the network or on the server: worth trying again by itself. */
val DepartureResult.isTransient: Boolean
    get() = this is DepartureResult.Pending && (reason == PendingReason.Network || reason == PendingReason.Server)

/**
 * The departure pending on this device. [wiping]: the proof came and the erasure runs (no button, not cancellable). [sinceMs]: time of the
 * last gesture, null for a marker of an older version. [last]: why the last attempt of this process is pending, null before the first.
 */
data class Departure(val kind: DepartureKind, val wiping: Boolean, val sinceMs: Long?, val last: PendingReason?)

/** What "Effacer quand même" gave. */
enum class EraseResult {
    /** Everything this device held is erased. */
    Erased,

    /**
     * The core refused: the answer of the relay that allowed it is no longer valid (older than 10 minutes, or replaced by another answer
     * while the user confirmed). Nothing was erased.
     */
    NotAllowed,

    /** The storage refused, or the call failed: nothing was erased. */
    Failed,
}
