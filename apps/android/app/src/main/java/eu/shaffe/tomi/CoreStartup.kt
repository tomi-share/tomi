package eu.shaffe.tomi

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Where the opening of the core stands. */
sealed interface StartupState<out T> {
    /** Being opened (or re-opened after a reset): Keystore, SQLite and the first read of the state, off the main thread. */
    data object Opening : StartupState<Nothing>

    data class Ready<out T>(val value: T) : StartupState<T>

    /**
     * The core could not be opened: the key that protects the database is gone (restored phone, cleared Keystore) or the database is
     * unreadable. [resetFailed] is true when a reset was asked and could not erase the local data either.
     */
    data class Unreadable(val resetFailed: Boolean = false) : StartupState<Nothing>
}

/**
 * Opens what the app needs ([open]: the Android Keystore, the SQLite database of the core...) OFF the main thread, and keeps the
 * outcome in [state] so that the screens wait for it, or offer a recovery, instead of the process dying at every launch.
 *
 * - [open] runs on [io]; any [Exception] (not an [Error]: a missing native library is a broken install, not damaged data) makes the
 *   state [StartupState.Unreadable], EXCEPT a [StartupBug] (the core is already open: see there), which crashes.
 * - [onReady] runs on [main] once [open] returned, before the state becomes [StartupState.Ready] (the container registers its lifecycle
 *   observers there, which must happen on the main thread). If it throws, the core is open and nothing can close it cleanly (its
 *   collectors run in the application scope): the exception is not caught, the app crashes, it does not become Unreadable.
 * - [retry] opens again (a Keystore that was not ready yet); [reset] erases the local data with [wipe], then opens again. Neither
 *   does anything unless the state is [StartupState.Unreadable].
 *
 * Plain Kotlin with injected dispatchers, so that it is tested without a device.
 */
class CoreStartup<T : Any>(
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    private val main: CoroutineDispatcher,
    private val open: () -> T,
    private val onReady: (T) -> Unit,
    private val wipe: () -> Unit,
) {
    private val mutableState = MutableStateFlow<StartupState<T>>(StartupState.Opening)
    val state: StateFlow<StartupState<T>> = mutableState.asStateFlow()

    /** Starts the first opening. Call once, from `Application.onCreate`. */
    fun begin() {
        scope.launch(io) { openNow() }
    }

    fun retry() {
        if (!leaveUnreadable()) return
        scope.launch(io) { openNow() }
    }

    /** Unreadable -> Opening, atomically; false (nothing done) in any other state. */
    private fun leaveUnreadable(): Boolean = mutableState.compareAndSet(StartupState.Unreadable(false), StartupState.Opening) ||
        mutableState.compareAndSet(StartupState.Unreadable(true), StartupState.Opening)

    /** Erases the local data of the core (it was unreadable) and opens a fresh one: the device is no longer in any space. */
    fun reset() {
        if (!leaveUnreadable()) return
        scope.launch(io) {
            try {
                wipe()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                mutableState.value = StartupState.Unreadable(resetFailed = true)
                return@launch
            }
            openNow()
        }
    }

    /** The container once the core is open; null when it could not be opened. Waits while it is being opened. */
    suspend fun await(): T? = (mutableState.first { it !is StartupState.Opening } as? StartupState.Ready)?.value

    private suspend fun openNow() {
        val value = try {
            open()
        } catch (e: CancellationException) {
            throw e
        } catch (e: StartupBug) {
            // Not damaged data: the native core is already open (collectors, database in use). Offering "Réessayer" would open a second
            // one on the same database and "Réinitialiser" would erase it under the first, so the failure is left to crash the app.
            throw e
        } catch (_: Exception) {
            // Nothing is logged: the message of a Keystore or storage failure may carry a path.
            mutableState.value = StartupState.Unreadable()
            return
        }
        // Outside the try on purpose: a failure of onReady (the container starting its observers and collectors) is a bug of the code,
        // with the core open. Same reason as StartupBug: it must not become Unreadable, it crashes.
        withContext(main) { onReady(value) }
        mutableState.value = StartupState.Ready(value)
    }
}

/**
 * Thrown by [CoreStartup]'s `open` when the core is ALREADY open and something after it failed (building the container): a bug of the
 * code, not damaged data. [CoreStartup] lets it crash the app instead of offering a retry or a reset that would run next to a core
 * that is still open.
 */
class StartupBug(cause: Throwable) : RuntimeException(cause)
