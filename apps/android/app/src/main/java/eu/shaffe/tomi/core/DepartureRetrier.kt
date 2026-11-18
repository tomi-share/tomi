package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.launch
import kotlin.random.Random

/** A refusal met by an automatic attempt: the user asked for this departure and must see that the relay refused it. */
data class DepartureRefusal(val kind: DepartureKind, val reason: RefusalReason)

/** What the screens of the departure need from the automatic attempts ([DepartureRetrier]). */
interface DepartureFollowUp {
    /** A refusal met by an automatic attempt, until a screen showed it ([consumeRefusal]). */
    val refusal: StateFlow<DepartureRefusal?>

    /** After a gesture that left the departure pending on the network or the server: (re)starts the series of automatic attempts. */
    fun kick()

    fun consumeRefusal()
}

/**
 * The automatic attempts of a pending departure (spec/api-v1.md 7.9.4): after a gesture, [TomiCore.continueDeparture] at
 * [DEPARTURE_RETRY_DELAYS_MS] (each plus 0 to 20 % at random) while it stays pending on the network or the server; then only when the
 * network comes back or the app comes back to the foreground, at most once a minute. One series at a time. The core decides what an
 * attempt sends: within 15 minutes of the last gesture it sends the request again, later it only verifies.
 *
 * Nothing here is logged (UniffiConfinementTest). [online] and [foreground] are the states of the network and of the app; [nowMs] a
 * monotonic clock.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class DepartureRetrier(
    private val core: TomiCore,
    private val scope: CoroutineScope,
    private val online: Flow<Boolean>,
    private val foreground: Flow<Boolean>,
    private val nowMs: () -> Long,
    private val random: Random = Random.Default,
    private val delaysMs: List<Long> = DEPARTURE_RETRY_DELAYS_MS,
) : DepartureFollowUp {
    private val lock = Any()
    private var series: Job? = null
    private var lastComeBackAttemptMs: Long? = null
    private val refusalState = MutableStateFlow<DepartureRefusal?>(null)

    override val refusal: StateFlow<DepartureRefusal?> = refusalState.asStateFlow()

    /** Watches the network and the foreground, only while a departure is pending. Call once. */
    fun start() {
        scope.launch {
            core.departure
                .map { it != null && !it.wiping }
                .distinctUntilChanged()
                .flatMapLatest { pending -> if (pending) merge(online.rises(), foreground.rises()) else emptyFlow() }
                .collect { onComeBack() }
        }
    }

    /** At start-up, before the pairing is settled: one automatic attempt, then the series if it stays pending on the network or server. */
    suspend fun settleAtStart(): DepartureResult {
        val result = automaticAttempt()
        if (result.isTransient) kick()
        return result
    }

    override fun kick() {
        synchronized(lock) {
            series?.cancel()
            series = scope.launch {
                for (base in delaysMs) {
                    delay(base + jitter(base))
                    if (!pending()) return@launch
                    if (!automaticAttempt().isTransient) return@launch
                }
            }
        }
    }

    override fun consumeRefusal() {
        refusalState.value = null
    }

    /** The network or the foreground came back: one attempt, unless one runs or a series is on, and at most one a minute. */
    private fun onComeBack() {
        synchronized(lock) {
            if (series?.isActive == true || core.departureRunning.value || !pending()) return
            val now = nowMs()
            val last = lastComeBackAttemptMs
            if (last != null && now - last < COME_BACK_MIN_GAP_MS) return
            lastComeBackAttemptMs = now
        }
        scope.launch { automaticAttempt() }
    }

    private suspend fun automaticAttempt(): DepartureResult {
        val kind = core.departure.value?.kind
        val result = core.continueDeparture()
        if (result is DepartureResult.Refused && kind != null) refusalState.value = DepartureRefusal(kind, result.reason)
        return result
    }

    private fun pending(): Boolean = core.departure.value?.let { !it.wiping } == true

    private fun jitter(base: Long): Long = (base * random.nextDouble() * MAX_JITTER).toLong()

    companion object {
        val DEPARTURE_RETRY_DELAYS_MS: List<Long> = listOf(2_000L, 5_000L, 15_000L, 30_000L, 60_000L, 120_000L)

        /** After the series: at most one attempt a minute when the network or the foreground comes back. */
        const val COME_BACK_MIN_GAP_MS = 60_000L

        private const val MAX_JITTER = 0.2
    }
}

/** One value each time the state goes from false to true (the first value is not a change). */
private fun Flow<Boolean>.rises(): Flow<Unit> = flow {
    var previous: Boolean? = null
    collect { now ->
        if (now && previous == false) emit(Unit)
        previous = now
    }
}
