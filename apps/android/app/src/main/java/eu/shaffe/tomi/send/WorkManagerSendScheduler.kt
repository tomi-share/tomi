package eu.shaffe.tomi.send

import android.content.Context
import android.os.SystemClock
import androidx.work.BackoffPolicy
import androidx.work.CoroutineWorker
import androidx.work.Constraints
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import eu.shaffe.tomi.TomiApplication
import eu.shaffe.tomi.core.DispatchResult
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import java.util.concurrent.TimeUnit

/** Durable local send queue: retries with back-off, and only runs when the network is available. */
class WorkManagerSendScheduler(context: Context) : SendScheduling {
    private val workManager = WorkManager.getInstance(context)

    override fun schedule(itemId: String, deadline: UndoDeadline) =
        enqueue(itemId, deadline, deadline.waitBeforeDispatchMs(SystemClock.elapsedRealtime()), ExistingWorkPolicy.REPLACE)

    override fun ensureScheduled(itemId: String) = enqueue(itemId, null, 0, ExistingWorkPolicy.KEEP)

    override fun cancel(itemId: String) {
        workManager.cancelUniqueWork(uniqueName(itemId))
    }

    private fun enqueue(itemId: String, deadline: UndoDeadline?, delayMs: Long, policy: ExistingWorkPolicy) {
        val data = Data.Builder().putString(SendWorker.KEY_ITEM_ID, itemId)
        if (deadline != null) {
            data.putLong(SendWorker.KEY_DEADLINE_ELAPSED_MS, deadline.atElapsedMs).putLong(SendWorker.KEY_WINDOW_MS, deadline.windowMs)
        }
        val request = OneTimeWorkRequestBuilder<SendWorker>()
            .setInputData(data.build())
            .setInitialDelay(delayMs, TimeUnit.MILLISECONDS)
            .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, BACKOFF_SECONDS, TimeUnit.SECONDS)
            .addTag(TAG)
            .build()
        workManager.enqueueUniqueWork(uniqueName(itemId), policy, request)
    }

    private fun uniqueName(itemId: String) = "send-$itemId"

    companion object {
        private const val BACKOFF_SECONDS = 15L

        /** Every send job; a job enqueued by an older version has no tag, and [SendWorker] ends it once the device is unpaired. */
        private const val TAG = "send"

        /** The device left its space: nothing queued can be sent any more. */
        fun cancelAll(context: Context) {
            WorkManager.getInstance(context).cancelAllWorkByTag(TAG)
        }
    }
}

/** Pushes one queued item to the relay through the core. The input only carries the item id, never content. */
class SendWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val itemId = inputData.getString(KEY_ITEM_ID) ?: return Result.failure()
        // WorkManager's initial delay uses the wall clock and may fire early: wait out the undo window ourselves,
        // on the same real-time deadline as the banner. Ignored when absent (resume after a restart) or stale (reboot).
        val values = inputData.keyValueMap
        val deadlineAt = values[KEY_DEADLINE_ELAPSED_MS] as? Long
        val windowMs = values[KEY_WINDOW_MS] as? Long
        if (deadlineAt != null && windowMs != null) {
            val deadline = UndoDeadline(deadlineAt, windowMs)
            val wait = deadline.waitBeforeDispatchMs(SystemClock.elapsedRealtime())
            if (wait > 0L) delay(wait)
        }
        // The core opens off the main thread, and a worker can be the first thing that runs in a new process. Unreadable data: nothing can be
        // sent; the job ends and the next start (which re-schedules what is queued) tries again.
        // failure() and not retry() on purpose: once the core is open (after "Réessayer", or at the next launch) AppContainer.start() runs
        // resumeQueuedSends(), which schedules again every item still queued; a back-off retry here would only wake a process that can do nothing.
        val core = (applicationContext as TomiApplication).awaitContainer()?.core ?: return Result.failure()
        // Left the space: nothing to send any more, and nothing to retry (the core would refuse every time).
        if (!core.isPaired.value) return Result.success()
        val result = try {
            core.dispatch(itemId)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // Core failure (storage, protocol): the item is still in the outbox, so try again with back-off.
            DispatchResult.RetryLater
        }
        return when (result) {
            DispatchResult.Sent, DispatchResult.NotFound -> Result.success()
            DispatchResult.RetryLater -> Result.retry()
        }
    }

    companion object {
        const val KEY_ITEM_ID = "item_id"
        const val KEY_DEADLINE_ELAPSED_MS = "deadline_elapsed_ms"
        const val KEY_WINDOW_MS = "window_ms"
    }
}
