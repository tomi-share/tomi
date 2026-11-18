package eu.shaffe.tomi.core

import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import uniffi.tomi_core.FfiInviteSession
import uniffi.tomi_core.TomiException
import java.util.concurrent.atomic.AtomicBoolean

/**
 * [InviteSession] over the UniFFI session of the Rust core (`FfiInviteSession`, crates/core/src/ffi.rs).
 *
 * The raw records of the core (`FfiVerifyPrompt`) carry the verification code in a `toString()`: each one is converted on the line that
 * receives it and goes out of scope there. The handle is released (`close()`) when the session is over, as the core asks: dropping it
 * closes nothing at the relay.
 */
internal class RealInviteSession(
    private val session: FfiInviteSession,
    private val bridge: BlockingBridge,
    private val retryDelaysMs: List<Long> = PairingErrors.DEFAULT_RETRY_DELAYS_MS,
) : InviteSession {
    private val released = AtomicBoolean(false)

    override val channelId: String = session.channelId()

    override fun code(): SensitiveText? = readWhileAlive { session.code() }?.let(::SensitiveText)

    override fun link(): SensitiveText? = readWhileAlive { session.link() }?.let(::SensitiveText)

    override fun expiresInMs(): Long = readWhileAlive { session.expiresInMs() } ?: 0L

    override suspend fun awaitPeer(): InvitePrompt {
        val prompt = try {
            bridge.call(onCancel = ::abort) { PairingErrors.retryingTransient(retryDelaysMs) { session.waitForPeer() } }
        } catch (e: TomiException) {
            release()
            throw PairingException(PairingErrors.failureOf(e), e)
        }
        return InvitePrompt(PairingPeer(prompt.peer.name, prompt.peer.platform.toDevicePlatform()), VerificationCode(prompt.sas))
    }

    override suspend fun confirm() {
        try {
            bridge.quick { session.confirm() }
        } catch (e: TomiException) {
            // "Not the moment" (InvalidInput) and the transient errors leave the session alive: the caller may try again.
            if (!PairingErrors.isTransient(e) && e !is TomiException.InvalidInput) release()
            throw PairingException(PairingErrors.failureOf(e), e)
        }
    }

    override suspend fun awaitDone(): InvitedDevice {
        val outcome = try {
            bridge.call(onCancel = ::abort) { PairingErrors.retryingTransient(retryDelaysMs) { session.waitDone() } }
        } catch (e: TomiException) {
            release()
            throw PairingException(PairingErrors.failureOf(e), e)
        }
        release()
        return InvitedDevice(outcome.deviceId, outcome.name, outcome.platform.toDevicePlatform(), historyAnnounced = outcome.history != null)
    }

    override suspend fun cancel() {
        withContext(NonCancellable) { bridge.quick { abort() } }
    }

    /** Closes the channel at the relay (a network call) and releases the handle. Idempotent. Blocks: background thread. */
    internal fun abort() {
        if (released.get()) return
        try {
            session.cancel()
        } catch (_: Exception) {
            // The session is being dropped anyway.
        }
        release()
    }

    private fun release() {
        if (!released.compareAndSet(false, true)) return
        try {
            session.close()
        } catch (_: Exception) {
            // Already destroyed.
        }
    }

    /** The snapshots answer at once, but a destroyed handle throws: a session that is over has nothing to show. */
    private fun <T> readWhileAlive(read: () -> T): T? {
        if (released.get()) return null
        return try {
            read()
        } catch (_: IllegalStateException) {
            null
        }
    }
}
