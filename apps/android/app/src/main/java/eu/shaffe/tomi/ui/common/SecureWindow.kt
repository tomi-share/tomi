package eu.shaffe.tomi.ui.common

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.os.Build
import android.view.View
import android.view.Window
import android.view.WindowManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalContext
import androidx.fragment.app.FragmentActivity
import java.util.WeakHashMap

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

/**
 * What FLAG_SECURE does not stop: a third-party autofill service receiving the value of a secret field in a save request, and an
 * accessibility service reading a revealed secret. While a window shows or composes a secret:
 * - autofill is switched off for the whole window (`IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS`, the Compose root included);
 * - on Android 14+ the window is flagged accessibility-data-sensitive: only services that declare `isAccessibilityTool` (TalkBack,
 *   switch access) still read it. On Android 13 (minSdk) only the autofill part exists.
 *
 * Not verified on a device: that Compose's virtual nodes inherit the flag from the decor view.
 */
private object SensitiveWindowProtection {
    private val previousAutofill = WeakHashMap<Window, Int>()

    fun apply(window: Window) {
        val decor = window.decorView
        previousAutofill[window] = decor.importantForAutofill
        decor.importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            decor.setAccessibilityDataSensitive(View.ACCESSIBILITY_DATA_SENSITIVE_YES)
        }
    }

    fun restore(window: Window) {
        val decor = window.decorView
        decor.importantForAutofill = previousAutofill.remove(window) ?: View.IMPORTANT_FOR_AUTOFILL_AUTO
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            decor.setAccessibilityDataSensitive(View.ACCESSIBILITY_DATA_SENSITIVE_AUTO)
        }
    }
}

/** Windows whose overlays are hidden ([FlagSecureEffect] `hideOverlays`): set by the first screen that asks, cleared by the last. */
private val overlayHiddenWindows = ReferenceCounter<Window>(
    onFirst = { it.setHideOverlayWindows(true) },
    onLast = { it.setHideOverlayWindows(false) },
)

/** One counter for the whole app, keyed by window: FLAG_SECURE (and the protection above) is set by the first user and cleared by the last. */
private val secureWindows = ReferenceCounter<Window>(
    onFirst = {
        it.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        SensitiveWindowProtection.apply(it)
    },
    onLast = {
        it.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
        SensitiveWindowProtection.restore(it)
    },
)

/**
 * Keeps FLAG_SECURE on the window while [enabled]: no screenshots, no screen recording, blank in the recents; also no autofill and,
 * on Android 14+, no accessibility service other than an accessibility tool ([SensitiveWindowProtection]).
 * Used whenever a secret is displayed in clear. Reference-counted per window, so two screens needing the flag
 * at once do not remove it from each other when one of them goes away.
 *
 * [hideOverlays] (pairing, invitation and creation screens, where a code is shown and a confirmation is tapped): also asks the system
 * to hide the windows that other apps draw on top of ours (`setHideOverlayWindows`, the normal permission `HIDE_OVERLAY_WINDOWS`), a
 * tapjacking hardening on top of the platform's untrusted-touch blocking. Same reference counting.
 */
@Composable
fun FlagSecureEffect(enabled: Boolean, hideOverlays: Boolean = false) {
    val context = LocalContext.current
    DisposableEffect(enabled, context) {
        val window = if (enabled) context.findActivity()?.window else null
        if (window != null) secureWindows.acquire(window)
        onDispose {
            if (window != null) secureWindows.release(window)
        }
    }
    DisposableEffect(hideOverlays, context) {
        val window = if (hideOverlays) context.findActivity()?.window else null
        if (window != null) overlayHiddenWindows.acquire(window)
        onDispose {
            if (window != null) overlayHiddenWindows.release(window)
        }
    }
}
