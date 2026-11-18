package eu.shaffe.tomi

import android.content.SharedPreferences
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class UserPrefsTest {
    @Test
    fun codeWrapIsOffByDefault() {
        assertFalse(UserPrefs.DEFAULT_CODE_WRAP)
        assertFalse(UserPrefs(MemoryPrefs()).codeWrapEnabled)
    }

    @Test
    fun codeWrapCanBeTurnedOnAndOff() {
        val prefs = UserPrefs(MemoryPrefs())
        prefs.codeWrapEnabled = true
        assertTrue(prefs.codeWrapEnabled)
        prefs.codeWrapEnabled = false
        assertFalse(prefs.codeWrapEnabled)
    }

    @Test
    fun codeWrapSurvivesANewSession() {
        // Same storage, new UserPrefs: what a restart of the app does.
        val storage = MemoryPrefs()
        UserPrefs(storage).codeWrapEnabled = true
        assertTrue(UserPrefs(storage).codeWrapEnabled)
    }

    @Test
    fun codeWrapUsesItsOwnKeyAndLeavesTheOtherPreferencesAlone() {
        val storage = MemoryPrefs()
        val prefs = UserPrefs(storage)
        prefs.onboardingDone = true
        prefs.codeWrapEnabled = true
        assertEquals(true, storage.getBoolean(UserPrefs.KEY_CODE_WRAP, false))
        assertTrue(prefs.onboardingDone)
        assertEquals(null, prefs.preferredRenderMode)
    }
}

/** In-memory SharedPreferences: the tests need the behaviour of the storage, not Android. */
internal class MemoryPrefs : SharedPreferences {
    private val values = HashMap<String, Any?>()

    override fun getAll(): MutableMap<String, *> = HashMap(values)
    override fun getString(key: String, defValue: String?): String? = values[key] as? String ?: defValue
    override fun getStringSet(key: String, defValues: MutableSet<String>?): MutableSet<String>? {
        @Suppress("UNCHECKED_CAST")
        return values[key] as? MutableSet<String> ?: defValues
    }
    override fun getInt(key: String, defValue: Int): Int = values[key] as? Int ?: defValue
    override fun getLong(key: String, defValue: Long): Long = values[key] as? Long ?: defValue
    override fun getFloat(key: String, defValue: Float): Float = values[key] as? Float ?: defValue
    override fun getBoolean(key: String, defValue: Boolean): Boolean = values[key] as? Boolean ?: defValue
    override fun contains(key: String): Boolean = values.containsKey(key)
    override fun edit(): SharedPreferences.Editor = MemoryEditor()
    override fun registerOnSharedPreferenceChangeListener(listener: SharedPreferences.OnSharedPreferenceChangeListener?) = Unit
    override fun unregisterOnSharedPreferenceChangeListener(listener: SharedPreferences.OnSharedPreferenceChangeListener?) = Unit

    private inner class MemoryEditor : SharedPreferences.Editor {
        private val pending = HashMap<String, Any?>()
        private val removed = HashSet<String>()
        private var clear = false

        override fun putString(key: String, value: String?) = put(key, value)
        override fun putStringSet(key: String, values: MutableSet<String>?) = put(key, values)
        override fun putInt(key: String, value: Int) = put(key, value)
        override fun putLong(key: String, value: Long) = put(key, value)
        override fun putFloat(key: String, value: Float) = put(key, value)
        override fun putBoolean(key: String, value: Boolean) = put(key, value)
        override fun remove(key: String): SharedPreferences.Editor = apply { removed.add(key) }
        override fun clear(): SharedPreferences.Editor = apply { clear = true }
        override fun commit(): Boolean {
            if (clear) values.clear()
            removed.forEach(values::remove)
            values.putAll(pending)
            return true
        }
        override fun apply() {
            commit()
        }

        private fun put(key: String, value: Any?): SharedPreferences.Editor = apply { pending[key] = value }
    }
}