package eu.shaffe.tomi

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import eu.shaffe.tomi.ui.render.RenderMode

/** Small on-device preferences. Never stores item content. */
class UserPrefs(private val prefs: SharedPreferences) {
    constructor(context: Context) : this(context.getSharedPreferences("tomi_prefs", Context.MODE_PRIVATE))

    /** Last rendering chosen for rich text; becomes the default for the next cards (docs D29). */
    var preferredRenderMode: RenderMode?
        get() = prefs.getString(KEY_RENDER_MODE, null)?.let { name -> RenderMode.entries.firstOrNull { it.name == name } }
        set(value) = prefs.edit { if (value == null) remove(KEY_RENDER_MODE) else putString(KEY_RENDER_MODE, value.name) }

    /**
     * Line wrap of the code file screen (`CodeFileScreen`): off = long lines scroll sideways, on = lines fold to the screen width.
     * One choice for every file, kept between sessions. Off by default (code reads better unfolded).
     */
    var codeWrapEnabled: Boolean
        get() = prefs.getBoolean(KEY_CODE_WRAP, DEFAULT_CODE_WRAP)
        set(value) = prefs.edit { putBoolean(KEY_CODE_WRAP, value) }

    var onboardingDone: Boolean
        get() = prefs.getBoolean(KEY_ONBOARDING_DONE, false)
        set(value) = prefs.edit { putBoolean(KEY_ONBOARDING_DONE, value) }

    /**
     * The device left its space while the app had no window focus: whether the clipboard still holds a clip of Tomi could not be told
     * then (Android only shows its description to the focused app). Checked, and cleared, at the next focus.
     */
    var clipboardClearPending: Boolean
        get() = prefs.getBoolean(KEY_CLIPBOARD_CLEAR_PENDING, false)
        set(value) = prefs.edit { putBoolean(KEY_CLIPBOARD_CLEAR_PENDING, value) }

    companion object {
        const val DEFAULT_CODE_WRAP = false
        const val KEY_CODE_WRAP = "code_wrap"
        private const val KEY_RENDER_MODE = "render_mode"
        private const val KEY_ONBOARDING_DONE = "onboarding_done"
        private const val KEY_CLIPBOARD_CLEAR_PENDING = "clipboard_clear_pending"
    }
}
