package eu.shaffe.tomi.notify

import android.Manifest
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import eu.shaffe.tomi.MainActivity
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.Device
import eu.shaffe.tomi.core.ItemKind
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.kind
import java.net.URI
import java.time.Clock
import java.time.Duration

/**
 * Builds and posts the notifications for received items. Hard rule: a secret's content is never put in a notification.
 * Real delivery will come from an FCM data message that wakes the core; this class only renders what the core received.
 */
class IncomingNotifications(
    private val context: Context,
    private val batcher: NotificationBatcher = NotificationBatcher(),
    private val clock: Clock = Clock.systemUTC(),
) {
    private val manager = NotificationManagerCompat.from(context)
    private val recent = LinkedHashMap<String, TomiItem>()

    @Synchronized
    fun show(item: TomiItem) {
        if (!canPost()) return
        val decision = batcher.onArrival(item.id, clock.millis())
        recent[item.id] = item
        while (recent.size > MAX_TRACKED) recent.remove(recent.keys.first())

        when (decision) {
            NotificationBatcher.Decision.Single -> post(notificationId(item.id), buildSingle(item).build())
            is NotificationBatcher.Decision.Summary -> {
                decision.itemIds.forEach { manager.cancel(notificationId(it)) }
                val batch = decision.itemIds.mapNotNull { recent[it] }
                post(SUMMARY_ID, buildSummary(batch, item).build())
            }
        }
    }

    /**
     * "Nouvel appareil ajouté": a detection control, shown on the devices that did not add it. Only the device name and platform
     * (metadata the space's members already see), never content; private on the lock screen, with a neutral public version.
     */
    fun showDeviceAdded(device: Device) {
        if (!canPost()) return
        post(deviceNotificationId(device.id), buildDeviceAdded(device).build())
    }

    fun cancel(itemId: String) {
        manager.cancel(notificationId(itemId))
    }

    /** Takes down the summary ("N nouveaux éléments"): its items were read or deleted elsewhere, nothing is left to announce. */
    @Synchronized
    fun cancelSummary() {
        manager.cancel(SUMMARY_ID)
    }

    /** Takes down every notification of Tomi, the summary included, and forgets the items they showed (the device left its space). */
    @Synchronized
    fun cancelAll() {
        manager.cancelAll()
        recent.clear()
    }

    private fun canPost(): Boolean =
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED &&
            manager.areNotificationsEnabled()

    private fun post(id: Int, notification: android.app.Notification) {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        manager.notify(id, notification)
    }

    internal fun buildSingle(item: TomiItem): NotificationCompat.Builder {
        val kind = item.kind()
        val from = item.sourceDevice.name
        val title = when (kind) {
            ItemKind.Secret -> context.getString(R.string.notif_title_secret, from)
            ItemKind.Link -> context.getString(R.string.notif_title_link, from)
            ItemKind.Image -> context.getString(R.string.notif_title_image, from)
            ItemKind.File -> context.getString(R.string.notif_title_file, from)
            ItemKind.RichText, ItemKind.Text -> context.getString(R.string.notif_title_text, from)
            ItemKind.Unreadable -> context.getString(R.string.notif_title_unreadable, from)
        }
        val body = when (kind) {
            // Never any hint of the content of a secret.
            ItemKind.Secret -> context.getString(R.string.notif_body_secret)
            ItemKind.Link -> item.title ?: hostOf(item.url) ?: context.getString(R.string.notif_body_generic)
            ItemKind.Image, ItemKind.File -> item.attachments.joinToString(", ") { it.name }.take(MAX_BODY)
            ItemKind.RichText, ItemKind.Text -> item.text?.plain?.take(MAX_BODY) ?: context.getString(R.string.notif_body_generic)
            ItemKind.Unreadable -> context.getString(R.string.notif_body_unreadable)
        }

        // An unreadable item may well be a secret: treat it like one on the lock screen.
        val builder = baseBuilder(NotificationChannels.channelFor(kind), secret = kind.isSecretLike())
            .setContentTitle(title)
            .setContentText(body)
            .setContentIntent(actionIntent(item.id, NotificationActionActivity.ACTION_TAP))

        secondaryAction(item)?.let { (label, action) ->
            builder.addAction(0, context.getString(label), actionIntent(item.id, action))
        }
        item.expiresAt?.let { expires ->
            val remaining = Duration.between(clock.instant(), expires)
            if (!remaining.isNegative && !remaining.isZero) builder.setTimeoutAfter(remaining.toMillis())
        }
        return builder
    }

    internal fun buildSummary(batch: List<TomiItem>, latest: TomiItem): NotificationCompat.Builder {
        val hasSecret = summaryHasSecret(batch, latest)
        val channel = if (hasSecret) NotificationChannels.SECRETS else NotificationChannels.channelFor(latest.kind())
        val count = maxOf(batch.size, 2)
        return baseBuilder(channel, secret = hasSecret)
            .setContentTitle(context.resources.getQuantityString(R.plurals.notif_summary_title, count, count))
            .setContentText(context.getString(R.string.notif_summary_text, latest.sourceDevice.name))
            .setContentIntent(openAppIntent())
    }

    internal fun buildDeviceAdded(device: Device): NotificationCompat.Builder {
        val publicVersion = NotificationCompat.Builder(context, NotificationChannels.DEVICES)
            .setSmallIcon(R.drawable.ic_stat_tomi)
            .setColor(ContextCompat.getColor(context, R.color.notification_accent))
            .setContentTitle(context.getString(R.string.notif_device_added_title))
            .build()
        return NotificationCompat.Builder(context, NotificationChannels.DEVICES)
            .setSmallIcon(R.drawable.ic_stat_tomi)
            .setColor(ContextCompat.getColor(context, R.color.notification_accent))
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .setAutoCancel(true)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(publicVersion)
            .setContentTitle(context.getString(R.string.notif_device_added_title))
            .setContentText(context.getString(R.string.notif_device_added_body, device.name))
            .setStyle(NotificationCompat.BigTextStyle().bigText(context.getString(R.string.notif_device_added_body, device.name)))
            .setContentIntent(openAppIntent())
    }

    private fun baseBuilder(channelId: String, secret: Boolean): NotificationCompat.Builder {
        val publicVersion = NotificationCompat.Builder(context, channelId)
            .setSmallIcon(R.drawable.ic_stat_tomi)
            .setColor(ContextCompat.getColor(context, R.color.notification_accent))
            .setContentTitle(context.getString(R.string.notif_public_title))
            .build()
        return NotificationCompat.Builder(context, channelId)
            .setSmallIcon(R.drawable.ic_stat_tomi)
            .setColor(ContextCompat.getColor(context, R.color.notification_accent))
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .setVisibility(if (secret) NotificationCompat.VISIBILITY_SECRET else NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(if (secret) null else publicVersion)
    }

    private fun secondaryAction(item: TomiItem): Pair<Int, String>? = if (!item.isReadable) null else when (TapAction.forItem(item)) {
        TapAction.OpenLink, TapAction.OpenCard -> R.string.notif_action_copy to NotificationActionActivity.ACTION_COPY
        TapAction.CopyText, TapAction.PreviewImage, TapAction.OpenFile -> R.string.notif_action_view to NotificationActionActivity.ACTION_VIEW_CARD
        TapAction.AuthenticateAndCopy -> null
    }

    private fun actionIntent(itemId: String, action: String): PendingIntent {
        val intent = Intent(context, NotificationActionActivity::class.java)
            .setAction(action)
            .putExtra(NotificationActionActivity.EXTRA_ITEM_ID, itemId)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
        return PendingIntent.getActivity(
            context,
            (itemId + action).hashCode(),
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }

    private fun openAppIntent(): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .setData(Uri.parse("tomi-share://open"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        return PendingIntent.getActivity(context, SUMMARY_ID, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun hostOf(url: String?): String? = try {
        url?.let { URI(it).host }
    } catch (_: Exception) {
        null
    }

    private fun notificationId(itemId: String): Int = itemId.hashCode()

    /** Distinct from an item's id (the same hash space) so that a device and an item can never replace each other. */
    private fun deviceNotificationId(deviceId: String): Int = ("device:" + deviceId).hashCode()

    internal companion object {
        /**
         * Whether the summary "N new items" must be treated as hidden (secret channel, nothing on the lock screen): a hidden item,
         * or an unreadable one, which may well be a secret (same rule as [buildSingle]).
         */
        fun summaryHasSecret(batch: List<TomiItem>, latest: TomiItem): Boolean =
            (batch + latest).any { it.kind().isSecretLike() }

        private fun ItemKind.isSecretLike(): Boolean = this == ItemKind.Secret || this == ItemKind.Unreadable

        const val SUMMARY_ID = 1
        const val MAX_BODY = 120
        const val MAX_TRACKED = 64
    }
}
