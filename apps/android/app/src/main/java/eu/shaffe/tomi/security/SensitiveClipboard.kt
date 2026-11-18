package eu.shaffe.tomi.security

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.os.PersistableBundle
import android.os.SystemClock
import androidx.work.Data
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit

/**
 * Writes to the clipboard. This class never READS the clipboard content (DEV.md invariant 2): the only read of
 * the clipboard in the whole app is the explicit "Paste" button of the compose screen.
 *
 * Secrets are flagged with EXTRA_IS_SENSITIVE (hidden from previews and clipboard history) and cleared after 30 s.
 */
class SensitiveClipboard(
    private val context: Context,
    private val elapsedRealtimeMs: () -> Long = { SystemClock.elapsedRealtime() },
) : SpaceClipboard {

    /** Whether a window of the app has focus (set by the activity): only then does Android show the clip description to the app. */
    @Volatile
    var windowHasFocus: Boolean = false

    /** Every clip of Tomi carries [LABEL], so that what Tomi put there can be told apart without reading it. */
    fun copy(plain: String, html: String? = null, sensitive: Boolean) {
        val manager = context.getSystemService(ClipboardManager::class.java)
        val clip = if (html != null) ClipData.newHtmlText(LABEL, plain, html) else ClipData.newPlainText(LABEL, plain)
        if (sensitive) {
            clip.description.extras = PersistableBundle().apply {
                putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
            }
        }
        manager.setPrimaryClip(clip)
        if (sensitive) {
            val timestamp = manager.primaryClipDescription?.timestamp ?: 0L
            pending = PendingClear(timestamp, elapsedRealtimeMs() + ClipboardClearPolicy.CLEAR_DELAY_MS)
            scheduleClear(timestamp)
        }
    }

    /**
     * Called when the app window gets focus. If the 30 s are over and the clipboard still holds the clip we put there
     * (the background job may not have run, or may have been unable to tell), clears it. Never clears a clip that is
     * not provably ours. The record is kept in memory only: after a process death the background job is the only
     * safety net. Not verified on a device.
     */
    fun clearIfStillOurs() {
        val record = pending ?: return
        val manager = context.getSystemService(ClipboardManager::class.java)
        val current = manager.primaryClipDescription?.timestamp
        when (ClipboardClearPolicy.onForeground(record.timestamp, current, elapsedRealtimeMs(), record.deadlineMs)) {
            ClipboardClearPolicy.ForegroundAction.Clear -> {
                manager.clearPrimaryClip()
                forgetPending(record.timestamp)
            }
            ClipboardClearPolicy.ForegroundAction.Forget -> forgetPending(record.timestamp)
            ClipboardClearPolicy.ForegroundAction.Wait, ClipboardClearPolicy.ForegroundAction.Retry -> Unit
        }
    }

    /**
     * The device left its space: a clip that Tomi put on the clipboard (its [LABEL], read from the description, never the content) goes now,
     * secret or not, without waiting for the 30 s. False when it cannot be told now (no window focus): the caller retries at the next focus.
     */
    override fun forgetSpace(): Boolean {
        if (!windowHasFocus) return false
        val manager = context.getSystemService(ClipboardManager::class.java)
        if (isTomiClip(manager.primaryClipDescription?.label)) manager.clearPrimaryClip()
        pending = null
        return true
    }

    private fun scheduleClear(timestamp: Long) {
        val request = OneTimeWorkRequestBuilder<ClearClipboardWorker>()
            .setInitialDelay(ClipboardClearPolicy.CLEAR_DELAY_MS, TimeUnit.MILLISECONDS)
            .setInputData(Data.Builder().putLong(KEY_TIMESTAMP, timestamp).build())
            .build()
        WorkManager.getInstance(context).enqueue(request)
    }

    internal class PendingClear(val timestamp: Long, val deadlineMs: Long)

    internal companion object {
        /** The label of every clip of Tomi. */
        const val LABEL = "Tomi"
        const val KEY_TIMESTAMP = "clip_ts"

        /** Whether a clip with this description label is one of Tomi (its [LABEL]); no label, or another one, is somebody else's: left alone. */
        fun isTomiClip(label: CharSequence?): Boolean = label?.toString() == LABEL

        /** The last secret copied and not yet cleared. The worker runs in the same process as the app. */
        @Volatile
        var pending: PendingClear? = null

        /** Stops tracking [timestamp], unless a newer secret has been copied since. */
        fun forgetPending(timestamp: Long) {
            if (pending?.timestamp == timestamp) pending = null
        }
    }
}

/** The clipboard as the cleanup after a departure sees it ([eu.shaffe.tomi.LeftSpaceCleanup]). */
fun interface SpaceClipboard {
    /** Clears the clip Tomi put on the clipboard, if it is still there. False when that cannot be told now (no window focus). */
    fun forgetSpace(): Boolean
}

/**
 * Clears the clipboard 30 s after a secret was copied, unless the user copied something else since.
 * In the background `primaryClipDescription` is expected to be null (Android 10+), in which case the clipboard is
 * cleared unconditionally: see [ClipboardClearPolicy]. This behaviour is by design and not verified on a device.
 */
class ClearClipboardWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val manager = applicationContext.getSystemService(ClipboardManager::class.java)
        val expected = inputData.getLong(SensitiveClipboard.KEY_TIMESTAMP, 0L)
        val current = manager.primaryClipDescription?.timestamp
        if (ClipboardClearPolicy.shouldClear(expected, current)) manager.clearPrimaryClip()
        SensitiveClipboard.forgetPending(expected)
        return Result.success()
    }
}
