package eu.shaffe.tomi.core

import kotlinx.coroutines.CancellationException

/**
 * Runs a call on the core without ever letting its failure reach the process: a network error, a storage error, a Keystore error or a
 * panic of the Rust side must be a message for the user, not a crash (the UI calls the core from `viewModelScope`, where an uncaught
 * exception ends the app).
 *
 * Returns true when [block] ended normally. Any [Exception] other than a cancellation runs [onFailure] (to tell the user) and returns
 * false; a cancellation is always rethrown, so structured concurrency keeps working. Nothing is logged: the message of a failure may
 * carry a path or an identifier, and the core handles secrets.
 */
suspend fun safeCore(onFailure: suspend () -> Unit = {}, block: suspend () -> Unit): Boolean = try {
    block()
    true
} catch (e: CancellationException) {
    throw e
} catch (_: Exception) {
    onFailure()
    false
}
