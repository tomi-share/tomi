package eu.shaffe.tomi.notify

import android.Manifest
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
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
import eu.shaffe.tomi.ui.common.formatFileSize
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

    /** The title of the notification of [item]: its type and the device it comes from, never anything of its content. */
    private fun titleOf(item: TomiItem): String {
        val from = item.sourceDevice.name
        val kind = item.kind()
        return when {
            kind == ItemKind.Secret -> context.getString(R.string.notif_title_secret, from)
            // Unreadable, or consumed by its first read without being hidden: nothing is said of the type.
            NotificationContent.isConcealed(item) -> context.getString(R.string.notif_title_unreadable, from)
            kind == ItemKind.Link -> context.getString(R.string.notif_title_link, from)
            kind == ItemKind.Image -> context.getString(R.string.notif_title_image, from)
            kind == ItemKind.File -> context.getString(R.string.notif_title_file, from)
            else -> context.getString(R.string.notif_title_text, from)
        }
    }

    internal fun buildSingle(item: TomiItem): NotificationCompat.Builder {
        val kind = item.kind()
        val concealed = NotificationContent.isConcealed(item)
        val channel = if (concealed && kind != ItemKind.Unreadable) NotificationChannels.SECRETS else NotificationChannels.channelFor(kind)
        // A concealed item (hidden, unreadable, consumed by its first read) is a secret on the lock screen, on a watch and in the history.
        val builder = baseBuilder(channel, secret = concealed)
            .setContentTitle(titleOf(item))
            .setContentIntent(actionIntent(item.id, NotificationActionActivity.ACTION_TAP))

        if (concealed) {
            // Never any hint of the content.
            builder.setContentText(
                context.getString(
                    when (kind) {
                        ItemKind.Secret -> R.string.notif_body_secret
                        ItemKind.Unreadable -> R.string.notif_body_unreadable
                        else -> R.string.notif_body_generic
                    },
                ),
            )
        } else {
            addContent(builder, item, kind)
            NotificationContent.buttonsFor(item).forEach { button ->
                val (label, action) = labelAndAction(button)
                builder.addAction(
                    NotificationCompat.Action.Builder(0, context.getString(label), actionIntent(item.id, action))
                        // The buttons read, copy, save or share the content: not before the phone is unlocked.
                        .setAuthenticationRequired(true)
                        .build(),
                )
            }
        }
        item.expiresAt?.let { expires ->
            val remaining = Duration.between(clock.instant(), expires)
            if (!remaining.isNegative && !remaining.isZero) {
                builder.setTimeoutAfter(remaining.toMillis())
                    // The notification counts down to its disappearance: a clock, not content.
                    .setWhen(expires.toEpochMilli())
                    .setShowWhen(true)
                    .setUsesChronometer(true)
                    .setChronometerCountDown(true)
            }
        }
        return builder
    }

    /** The rich part of the notification of a readable item that is not concealed: its body, and the style of its type. */
    private fun addContent(builder: NotificationCompat.Builder, item: TomiItem, kind: ItemKind) {
        val generic = context.getString(R.string.notif_body_generic)
        when (kind) {
            ItemKind.Link -> {
                // The title if there is one, else the host. The host (punycode, no path or query: they may carry a token) also shows
                // under the title.
                val host = NotificationContent.hostOf(item.url)
                val title = item.title?.takeIf { it.isNotBlank() }?.let { NotificationContent.bounded(it.trim(), NotificationContent.MAX_BODY) }
                builder.setContentText(title ?: host ?: generic)
                if (title != null && host != null) builder.setSubText(host)
            }
            ItemKind.Image, ItemKind.File -> {
                val names = item.attachments.joinToString(", ") { it.name }
                builder.setContentText(NotificationContent.bounded(names, NotificationContent.MAX_BODY).ifEmpty { generic })
                builder.setSubText(formatFileSize(context, item.attachments.sumOf { it.size }))
                if (kind == ItemKind.Image) pictureOf(item)?.let { picture ->
                    builder.setLargeIcon(picture)
                    builder.setStyle(NotificationCompat.BigPictureStyle().bigPicture(picture).bigLargeIcon(null as Bitmap?))
                }
            }
            else -> {
                builder.setContentText(NotificationContent.collapsedText(item) ?: generic)
                NotificationContent.expandedText(item)?.let { builder.setStyle(NotificationCompat.BigTextStyle().bigText(it)) }
            }
        }
    }

    /**
     * The thumbnail the sender sealed in the first image of the item, decoded IN MEMORY only (never written anywhere) and reduced to
     * [NotificationContent.MAX_PICTURE_EDGE]. The original image is never decrypted for a notification. Null when there is none, or when
     * it is too large or not an image: a hostile thumbnail must not cost memory.
     */
    private fun pictureOf(item: TomiItem): Bitmap? {
        val bytes = item.attachments.firstOrNull { it.isImage && it.thumbnail != null }?.thumbnail ?: return null
        if (bytes.size > NotificationContent.MAX_THUMBNAIL_BYTES) return null
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0 || bounds.outWidth.toLong() * bounds.outHeight > MAX_PICTURE_PIXELS) return null
        val options = BitmapFactory.Options().apply {
            inSampleSize = NotificationContent.sampleSizeFor(bounds.outWidth, bounds.outHeight)
        }
        return try {
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
        } catch (_: RuntimeException) {
            null
        }
    }

    internal fun buildSummary(batch: List<TomiItem>, latest: TomiItem): NotificationCompat.Builder {
        val hasSecret = summaryHasSecret(batch, latest)
        val channel = if (hasSecret) NotificationChannels.SECRETS else NotificationChannels.channelFor(latest.kind())
        val count = maxOf(batch.size, 2)
        val title = context.resources.getQuantityString(R.plurals.notif_summary_title, count, count)
        val builder = baseBuilder(channel, secret = hasSecret)
            .setContentTitle(title)
            .setContentText(context.getString(R.string.notif_summary_text, latest.sourceDevice.name))
            .setContentIntent(openAppIntent())
        if (!hasSecret) {
            // Expanded: one line per item, its type and its device (the title of its own notification), never its content.
            val lines = NotificationCompat.InboxStyle().setBigContentTitle(title)
            batch.takeLast(NotificationContent.MAX_SUMMARY_LINES).forEach { lines.addLine(titleOf(it)) }
            builder.setStyle(lines)
        }
        return builder
    }

    internal fun buildDeviceAdded(device: Device): NotificationCompat.Builder {
        // The name comes from the device itself: shown bounded, like the body of an item.
        val body = context.getString(R.string.notif_device_added_body, device.name.take(MAX_BODY))
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
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
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
            // The system adds no button of its own ("open this link", "copy this code"): it would read the content we keep out of it.
            .setAllowSystemGeneratedContextualActions(false)
            // A concealed item stays on this phone: not bridged to a paired watch or another device.
            .setLocalOnly(secret)
    }

    private fun labelAndAction(button: NotificationButton): Pair<Int, String> = when (button) {
        NotificationButton.Copy -> R.string.notif_action_copy to NotificationActionActivity.ACTION_COPY
        NotificationButton.View -> R.string.notif_action_view to NotificationActionActivity.ACTION_VIEW_CARD
        NotificationButton.Save -> R.string.action_save to NotificationActionActivity.ACTION_SAVE
        NotificationButton.Share -> R.string.action_share to NotificationActionActivity.ACTION_SHARE
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

    private fun notificationId(itemId: String): Int = itemId.hashCode()

    /** Distinct from an item's id (the same hash space) so that a device and an item can never replace each other. */
    private fun deviceNotificationId(deviceId: String): Int = ("device:" + deviceId).hashCode()

    internal companion object {
        /**
         * Whether the summary "N new items" must be treated as hidden (secret channel, nothing on the lock screen): a hidden item,
         * an unreadable one (which may well be a secret) or one consumed by its first read (same rule as [buildSingle]).
         */
        fun summaryHasSecret(batch: List<TomiItem>, latest: TomiItem): Boolean =
            (batch + latest).any(NotificationContent::isConcealed)

        const val SUMMARY_ID = 1
        const val MAX_BODY = NotificationContent.MAX_BODY
        const val MAX_TRACKED = 64

        /** A thumbnail with more pixels than this is not decoded for a notification (16 megapixels). */
        private const val MAX_PICTURE_PIXELS = 16L * 1024 * 1024
    }
}
