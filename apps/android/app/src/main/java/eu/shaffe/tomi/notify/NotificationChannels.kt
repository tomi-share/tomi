package eu.shaffe.tomi.notify

import android.app.NotificationChannel
import android.app.NotificationChannelGroup
import android.app.NotificationManager
import android.content.Context
import androidx.core.app.NotificationCompat
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.ItemKind

/** One notification channel per content type so the user can tune each one (and silence links but not secrets). */
object NotificationChannels {
    const val LINKS = "incoming_links"
    const val TEXT = "incoming_text"
    const val FILES = "incoming_files"
    const val SECRETS = "incoming_secrets"
    const val DEVICES = "space_devices"
    private const val GROUP = "incoming"

    fun channelFor(kind: ItemKind): String = when (kind) {
        ItemKind.Secret -> SECRETS
        ItemKind.Link -> LINKS
        ItemKind.Image, ItemKind.File -> FILES
        ItemKind.RichText, ItemKind.Text, ItemKind.Unreadable -> TEXT
    }

    fun create(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannelGroup(
            NotificationChannelGroup(GROUP, context.getString(R.string.channel_group_incoming)),
        )
        manager.createNotificationChannels(
            listOf(
                channel(context, LINKS, R.string.channel_links, R.string.channel_links_desc),
                channel(context, TEXT, R.string.channel_text, R.string.channel_text_desc),
                channel(context, FILES, R.string.channel_files, R.string.channel_files_desc),
                // Outside the "received items" group: it is an alert about the space, not an incoming item. Private on the lock screen.
                NotificationChannel(DEVICES, context.getString(R.string.channel_devices), NotificationManager.IMPORTANCE_HIGH).apply {
                    description = context.getString(R.string.channel_devices_desc)
                    lockscreenVisibility = NotificationCompat.VISIBILITY_PRIVATE
                    setShowBadge(false)
                },
                channel(context, SECRETS, R.string.channel_secrets, R.string.channel_secrets_desc).apply {
                    // Secrets never show on the lock screen, and their content is never in a notification anyway.
                    lockscreenVisibility = NotificationCompat.VISIBILITY_SECRET
                    setShowBadge(false)
                },
            ),
        )
    }

    private fun channel(context: Context, id: String, name: Int, description: Int) =
        NotificationChannel(id, context.getString(name), NotificationManager.IMPORTANCE_DEFAULT).apply {
            this.description = context.getString(description)
            group = GROUP
        }
}
