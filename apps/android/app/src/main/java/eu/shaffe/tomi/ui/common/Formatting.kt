package eu.shaffe.tomi.ui.common

import android.content.Context
import android.text.format.DateFormat
import android.text.format.Formatter
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DeliveryStatus
import eu.shaffe.tomi.share.ExpiryChoice
import kotlinx.coroutines.delay
import java.time.Duration
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/**
 * A date formatter for the language of the device, from a skeleton ("dMMM": day and short month, in the order and
 * with the punctuation that language wants).
 */
private fun localized(skeleton: String): DateTimeFormatter {
    val locale = Locale.getDefault()
    return DateTimeFormatter.ofPattern(DateFormat.getBestDateTimePattern(locale, skeleton), locale)
}

/** Compact timestamp: time today, "yesterday", otherwise the short date. */
fun formatTimestamp(context: Context, instant: Instant, now: Instant = Instant.now()): String {
    val zone = ZoneId.systemDefault()
    val date = instant.atZone(zone).toLocalDate()
    val today = now.atZone(zone).toLocalDate()
    return when {
        date == today -> DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(Locale.getDefault()).format(instant.atZone(zone))
        date == today.minusDays(1) -> context.getString(R.string.time_yesterday)
        else -> localized("dMMM").format(instant.atZone(zone))
    }
}

/** Full date and time for the detail screen, e.g. "Wed, Nov 5, 13:47" (in the language of the device). */
fun formatDateTime(instant: Instant): String =
    localized("EEEdMMMHHmm").format(instant.atZone(ZoneId.systemDefault()))

/** Exact send date for the detail screen, with seconds, e.g. "November 5, 2026 at 13:47:09". */
fun formatPreciseDateTime(instant: Instant): String =
    localized("dMMMMyyyyHHmmss").format(instant.atZone(ZoneId.systemDefault()))

fun formatFileSize(context: Context, bytes: Long): String = Formatter.formatShortFileSize(context, bytes)

/** Remaining lifetime as a short string such as "2 h" or "3 j"; null if already expired. */
@Composable
fun formatRemaining(expiresAt: Instant?, now: Instant = Instant.now()): String? {
    if (expiresAt == null) return null
    val remaining = Duration.between(now, expiresAt)
    if (remaining.isNegative || remaining.isZero) return null
    return when {
        remaining.toDays() >= 1 -> pluralStringResource(R.plurals.remaining_days, remaining.toDays().toInt(), remaining.toDays().toInt())
        remaining.toHours() >= 1 -> pluralStringResource(R.plurals.remaining_hours, remaining.toHours().toInt(), remaining.toHours().toInt())
        else -> pluralStringResource(R.plurals.remaining_minutes, maxOf(1, remaining.toMinutes().toInt()), maxOf(1, remaining.toMinutes().toInt()))
    }
}

/**
 * How long to wait before [formatRemaining] can show something else: the time left to the next unit boundary
 * (day, hour, minute), at least 1 s (no churn) and at most 60 s (a sleeping device or a clock change shifts the
 * boundary; the loop then catches up on the next tick). Null when there is nothing to refresh (no expiry, or past).
 */
fun remainingRefreshDelay(expiresAt: Instant?, now: Instant): Duration? {
    if (expiresAt == null) return null
    val left = Duration.between(now, expiresAt)
    if (left.isNegative || left.isZero) return null
    val unit = when {
        left >= Duration.ofDays(1) -> Duration.ofDays(1)
        left >= Duration.ofHours(1) -> Duration.ofHours(1)
        else -> Duration.ofMinutes(1)
    }
    val untilChange = Duration.ofMillis(left.toMillis() % unit.toMillis() + 1)
    return untilChange.coerceIn(Duration.ofSeconds(1), Duration.ofSeconds(60))
}

private fun Duration.coerceIn(min: Duration, max: Duration): Duration = if (this < min) min else if (this > max) max else this

/**
 * The current time, re-read whenever the text of [formatRemaining] for [expiresAt] can change, so a card keeps
 * "expire dans 2 heures" up to date by itself. Ticks only while the screen is visible (STARTED) and re-reads the
 * clock when it comes back; a value that never expires costs nothing.
 */
@Composable
fun rememberNow(expiresAt: Instant?): Instant {
    var now by remember { mutableStateOf(Instant.now()) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(expiresAt, lifecycle) {
        if (expiresAt == null) return@LaunchedEffect
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                now = Instant.now()
                val wait = remainingRefreshDelay(expiresAt, now) ?: break
                delay(wait.toMillis())
            }
        }
    }
    return now
}

@Composable
fun expiryLabel(choice: ExpiryChoice): String = stringResource(
    when (choice) {
        ExpiryChoice.Hour1 -> R.string.expiry_hour1
        ExpiryChoice.Day1 -> R.string.expiry_day1
        ExpiryChoice.Day7 -> R.string.expiry_day7
        ExpiryChoice.Day30 -> R.string.expiry_day30
        ExpiryChoice.Never -> R.string.expiry_never
        ExpiryChoice.AfterFirstRead -> R.string.expiry_after_read
    },
)

@Composable
fun deliveryLabel(status: DeliveryStatus, isOnline: Boolean): String = when (status) {
    DeliveryStatus.Received -> stringResource(R.string.status_received)
    DeliveryStatus.Queued ->
        stringResource(if (isOnline) R.string.status_queued else R.string.status_waiting_network)
    DeliveryStatus.Sent -> stringResource(R.string.status_sent)
    DeliveryStatus.Failed -> stringResource(R.string.status_failed)
    is DeliveryStatus.Delivered ->
        if (status.delivered == status.total) {
            stringResource(R.string.status_delivered_all)
        } else {
            stringResource(R.string.status_delivered_partial, status.delivered, status.total)
        }
    is DeliveryStatus.ExpiredBeforeDelivery ->
        stringResource(R.string.status_expired, status.delivered, status.deviceNames.joinToString(", "))
}
