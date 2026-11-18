package eu.shaffe.tomi.ui.common

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.view.Window
import android.view.WindowManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalContext
import androidx.fragment.app.FragmentActivity

fun Context.findActivity(): Activity? {
    var ctx: Context? = this
    while (ctx is ContextWrapper) {
        if (ctx is Activity) return ctx
        ctx = ctx.baseContext
    }
    return null
}

fun Context.findFragmentActivity(): FragmentActivity? = findActivity() as? FragmentActivity

/**
 * Counts how many users currently need a resource per key, and tells when the first one arrives ([onFirst]) and
 * when the last one leaves ([onLast]). Used for FLAG_SECURE: several screens of the same window can show a secret
 * at the same time (feed + compose screen), and the flag must stay until none of them needs it.
 *
 * Main thread only in practice (Compose effects); synchronised anyway because it is cheap.
 */
class ReferenceCounter<K : Any>(
    private val onFirst: (K) -> Unit,
    private val onLast: (K) -> Unit,
) {
    private val counts = HashMap<K, Int>()

    @Synchronized
    fun acquire(key: K) {
        val next = (counts[key] ?: 0) + 1
        counts[key] = next
        if (next == 1) onFirst(key)
    }

    /** An unbalanced release (no matching [acquire]) is ignored. */
    @Synchronized
    fun release(key: K) {
        val current = counts[key] ?: return
        if (current <= 1) {
            counts.remove(key)
            onLast(key)
        } else {
            counts[key] = current - 1
        }
    }

    @Synchronized
    fun count(key: K): Int = counts[key] ?: 0
}

/** One counter for the whole app, keyed by window: FLAG_SECURE is set by the first user and cleared by the last. */
private val secureWindows = ReferenceCounter<Window>(
    onFirst = { it.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE) },
    onLast = { it.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) },
)

/**
 * Keeps FLAG_SECURE on the window while [enabled]: no screenshots, no screen recording, blank in the recents.
 * Used whenever a secret is displayed in clear. Reference-counted per window, so two screens needing the flag
 * at once do not remove it from each other when one of them goes away.
 */
@Composable
fun FlagSecureEffect(enabled: Boolean) {
    val context = LocalContext.current
    DisposableEffect(enabled, context) {
        val window = if (enabled) context.findActivity()?.window else null
        if (window != null) secureWindows.acquire(window)
        onDispose {
            if (window != null) secureWindows.release(window)
        }
    }
}
