package eu.shaffe.tomi.ui.pairing

import android.os.SystemClock
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlinx.coroutines.delay
import java.util.Locale

/** The countdown of a pairing code, "1:48". */
object Countdown {
    private const val SECOND_MS = 1_000L
    private const val MINUTE_S = 60L

    /** Minutes and seconds, rounded UP to the second: it reads 0:01 until the very end, never 0:00 while time is left. */
    fun format(remainingMs: Long): String {
        val seconds = (remainingMs.coerceAtLeast(0L) + SECOND_MS - 1) / SECOND_MS
        return String.format(Locale.ROOT, "%d:%02d", seconds / MINUTE_S, seconds % MINUTE_S)
    }
}

/**
 * Milliseconds left until [deadlineElapsedMs] (`SystemClock.elapsedRealtime()` time base, so that a change of the clock of the phone
 * does not move it), refreshed every second, 0 from the deadline on. Stays at 0: reaching it ends nothing here, what happens at the
 * deadline is the caller's business.
 */
@Composable
fun rememberCountdownMs(deadlineElapsedMs: Long): Long {
    var remaining by remember(deadlineElapsedMs) { mutableLongStateOf((deadlineElapsedMs - SystemClock.elapsedRealtime()).coerceAtLeast(0L)) }
    LaunchedEffect(deadlineElapsedMs) {
        while (true) {
            remaining = (deadlineElapsedMs - SystemClock.elapsedRealtime()).coerceAtLeast(0L)
            if (remaining == 0L) break
            delay(1_000L)
        }
    }
    return remaining
}
