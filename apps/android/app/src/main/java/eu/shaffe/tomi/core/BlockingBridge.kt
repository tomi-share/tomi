package eu.shaffe.tomi.core

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import uniffi.tomi_core.InternalException
import uniffi.tomi_core.TomiException

/**
 * Runs the blocking calls of the Rust core (SQLite, network) off the caller's thread.
 *
 * Every call of the UniFFI object blocks. A session of the pairing waits for the other person (long polls, minutes at most) and a blocked
 * call cannot be interrupted from Kotlin: only the session's own `cancel()` ends it ("callable from any thread, also while another call
 * of this session is blocked", crates/core/src/ffi.rs). [call] ties the two together: when the coroutine that waits is cancelled it
 * returns to its caller at once and runs [call]'s `onCancel` in the background, which unblocks the thread.
 *
 * What the generated bindings throw besides [TomiException] is turned into one, so that the callers have one kind of failure to handle:
 * a panic of the core (`InternalException`) is an internal error, and a call on a session that the app already released is a cancelled
 * pairing.
 */
internal class BlockingBridge(private val scope: CoroutineScope, private val io: CoroutineDispatcher) {
    /** A short blocking call (one local read, one request): not interruptible, the caller is released when it ends. */
    suspend fun <T> quick(block: () -> T): T = withContext(io) { translating(block) }

    /**
     * Like [quick], and it also runs to its end when the caller is cancelled meanwhile: for the call that creates a session at the relay,
     * whose result must reach the caller's cleanup instead of being dropped (a session nobody holds cannot be closed).
     */
    suspend fun <T> toTheEnd(block: () -> T): T = withContext(io + NonCancellable) { translating(block) }

    /**
     * A blocking call that may wait for a long time. Cancelling the caller releases it at once (CancellationException) and runs
     * [onCancel] in the background; whatever [block] returns or throws after that is dropped.
     */
    suspend fun <T> call(onCancel: () -> Unit, block: () -> T): T = suspendCancellableCoroutine { continuation ->
        continuation.invokeOnCancellation { inBackground(onCancel) }
        scope.launch(io) {
            val outcome = try {
                Result.success(translating(block))
            } catch (e: Exception) {
                Result.failure(e)
            }
            // Ignored when the caller was cancelled in the meantime.
            continuation.resumeWith(outcome)
        }
    }

    /** Runs [block] on the IO threads, outside the caller's lifetime; a failure is dropped (this is cleanup). */
    fun inBackground(block: () -> Unit) {
        scope.launch(io) {
            try {
                block()
            } catch (_: Exception) {
                // Best effort: a session that cannot be closed at the relay expires there by itself.
            }
        }
    }

    private fun <T> translating(block: () -> T): T = try {
        block()
    } catch (e: CancellationException) {
        throw e
    } catch (_: InternalException) {
        throw TomiException.Internal("the core failed unexpectedly")
    } catch (e: IllegalStateException) {
        // The generated objects refuse a call once they were closed ("object has already been destroyed").
        if (e.message?.contains("already been destroyed") == true) throw TomiException.Crypto("pairing_cancelled")
        throw e
    }
}
