package eu.shaffe.tomi.share

import android.content.Context
import android.content.Intent
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import eu.shaffe.tomi.MainActivity
import eu.shaffe.tomi.R

/**
 * Direct Share target: a long-lived dynamic shortcut bound to the share-target declared in res/xml/shortcuts.xml.
 * Android decides the final ranking in the share sheet; reporting each use is what keeps Tomi at the top.
 */
object SharingShortcut {
    const val ID = "tomi_send"
    const val CATEGORY = "eu.shaffe.tomi.category.SHARE_TARGET"

    fun publish(context: Context) {
        val shortcut = ShortcutInfoCompat.Builder(context, ID)
            .setShortLabel(context.getString(R.string.shortcut_short_label))
            .setLongLabel(context.getString(R.string.shortcut_long_label))
            .setIcon(IconCompat.createWithResource(context, R.mipmap.ic_launcher))
            .setIntent(Intent(context, MainActivity::class.java).setAction(Intent.ACTION_VIEW))
            .setCategories(setOf(CATEGORY))
            .setLongLived(true)
            .setRank(0)
            .build()
        ShortcutManagerCompat.pushDynamicShortcut(context, shortcut)
    }

    /** The device is not in a space: Tomi is no longer offered as a direct share target. */
    fun remove(context: Context) {
        ShortcutManagerCompat.removeDynamicShortcuts(context, listOf(ID))
    }

    fun reportUsed(context: Context) {
        ShortcutManagerCompat.reportShortcutUsed(context, ID)
    }
}
