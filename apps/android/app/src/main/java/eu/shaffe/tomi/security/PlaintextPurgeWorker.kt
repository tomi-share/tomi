package eu.shaffe.tomi.security

import android.content.Context
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import eu.shaffe.tomi.TomiApplication
import kotlinx.coroutines.CancellationException
import java.util.concurrent.TimeUnit

/**
 * Guarantees that the decrypted copies of the attachments (`files/cache`, see AGENTS.md rule 3) are erased after the app went to
 * the background, even if the process dies before the in-process timer fires (low memory right after "Ouvrir avec" is common). It is
 * scheduled at [DELAY_MS] when the app stops and cancelled when it comes back; WorkManager restarts the process if need be, the core
 * purges the cache when it opens, and the explicit call below covers a core that was already open.
 */
class PlaintextPurgeWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        // Back on screen meanwhile (the cancellation can lose the race with the start of the job): the user may be reading a copy.
        if (ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) return Result.success()
        // The core cannot open (unreadable data): there is nothing of ours to purge from here.
        val container = (applicationContext as TomiApplication).awaitContainer() ?: return Result.success()
        return try {
            container.core.purgePlaintextCache()
            Result.success()
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // A file still open in another app: try again later.
            Result.retry()
        }
    }

    companion object {
        /** After the 60 s of the in-process timer: this job only matters when that timer did not get to run. */
        const val DELAY_MS = 2 * 60_000L
        private const val UNIQUE_NAME = "purge_plaintext_cache"

        /** The app went to the background: one job, replacing any previous one. */
        fun schedule(context: Context) {
            val request = OneTimeWorkRequestBuilder<PlaintextPurgeWorker>()
                .setInitialDelay(DELAY_MS, TimeUnit.MILLISECONDS)
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork(UNIQUE_NAME, ExistingWorkPolicy.REPLACE, request)
        }

        /** The app is back on screen: the in-process timer is cancelled too, and so is this job. */
        fun cancel(context: Context) {
            WorkManager.getInstance(context).cancelUniqueWork(UNIQUE_NAME)
        }
    }
}
