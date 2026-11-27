package eu.shaffe.tomi.notify

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.core.app.NotificationManagerCompat
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.lifecycleScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.MainActivity
import eu.shaffe.tomi.TomiApplication
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.ItemKind
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.kind
import eu.shaffe.tomi.core.safeCore
import eu.shaffe.tomi.core.slotOf
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.share.FileDownloadResult
import eu.shaffe.tomi.share.FileDownloads
import eu.shaffe.tomi.share.FileShareOutcome
import eu.shaffe.tomi.share.FileSharing
import eu.shaffe.tomi.share.ItemSharing
import eu.shaffe.tomi.share.LinkExtractor
import eu.shaffe.tomi.share.ShareOutcome
import eu.shaffe.tomi.ui.common.launchFileShare
import eu.shaffe.tomi.ui.common.launchSystemShare
import eu.shaffe.tomi.ui.common.saveToDownloads
import eu.shaffe.tomi.ui.render.RenderModes
import eu.shaffe.tomi.ui.viewer.ImageExport
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Invisible activity that performs the "obvious action" when a notification (or one of its buttons) is tapped.
 * It exists so that a biometric prompt can be shown for secrets without opening the app (docs D25).
 */
class NotificationActionActivity : FragmentActivity() {

    /** Set once the core is open (the activity waits for it in [onCreate]); never read before. */
    private lateinit var container: AppContainer

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val itemId = intent.getStringExtra(EXTRA_ITEM_ID)?.takeIf { it.length in 1..MAX_ID_LENGTH }
        if (itemId == null) {
            finish()
            return
        }
        NotificationManagerCompat.from(this).cancel(itemId.hashCode())
        lifecycleScope.launch {
            try {
                // The core opens off the main thread: wait for it. Unreadable data: the app itself offers the way out, say so and close.
                container = (application as TomiApplication).awaitContainer() ?: run {
                    Toast.makeText(this@NotificationActionActivity, R.string.share_unreadable, Toast.LENGTH_LONG).show()
                    return@launch
                }
                val item = container.core.getItem(itemId)
                if (item == null) {
                    Toast.makeText(this@NotificationActionActivity, R.string.toast_item_not_found, Toast.LENGTH_SHORT).show()
                } else {
                    perform(item, intent.action)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // A failure of the core (storage, Keystore...) must not end the app from a notification: say so and close.
                Toast.makeText(this@NotificationActionActivity, R.string.snackbar_action_failed, Toast.LENGTH_SHORT).show()
            } finally {
                finish()
            }
        }
    }

    /** "Read" is bookkeeping after an action that worked: a failure of the core leaves the item unread, nothing more. */
    private suspend fun markReadQuietly(itemId: String) {
        safeCore { container.core.markRead(itemId) }
    }

    private suspend fun perform(item: TomiItem, action: String?) {
        when (action) {
            // A concealed item has no button (an older notification, or a forged intent, may still ask): the card is the only way in,
            // where the biometric gate and the "after first read" rule live.
            ACTION_COPY, ACTION_SAVE, ACTION_SHARE -> if (NotificationContent.isConcealed(item)) {
                openCard(item.id)
            } else when (action) {
                ACTION_COPY -> copy(item)
                ACTION_SAVE -> save(item)
                else -> share(item)
            }
            ACTION_VIEW_CARD -> openCard(item.id)
            else -> when (TapAction.forItem(item)) {
                TapAction.OpenLink -> openLink(item)
                TapAction.CopyText, TapAction.AuthenticateAndCopy -> copy(item)
                TapAction.OpenCard, TapAction.PreviewImage -> openCard(item.id)
                TapAction.OpenFile -> openFile(item)
            }
        }
    }

    private suspend fun copy(item: TomiItem) {
        if (item.secret) {
            val outcome = container.biometricGate.authenticate(
                activity = this,
                title = getString(R.string.biometric_title_copy_secret),
                subtitle = getString(R.string.biometric_subtitle_from_notification),
                cancelLabel = getString(R.string.action_cancel),
            )
            if (outcome != AuthOutcome.Success) {
                if (outcome == AuthOutcome.Unavailable) {
                    Toast.makeText(this, R.string.toast_biometric_unavailable, Toast.LENGTH_LONG).show()
                }
                return
            }
        }
        val text = item.text
        val url = item.url
        val value: String
        var html: String? = null
        if (url != null && !item.secret) {
            value = url
        } else if (text != null) {
            val content = RenderModes.copyContent(text, RenderModes.resolve(text, container.prefs.preferredRenderMode))
            value = content.plain
            html = content.html
        } else if (url != null) {
            // A hidden item that holds only a link (authenticated above): the link, like the feed does.
            value = url
        } else {
            return
        }
        container.clipboard.copy(value, html, sensitive = item.secret)
        markReadQuietly(item.id)
        // No app Toast here: this activity has no UI to show the feedback of the app (ui/common/Feedback.kt), and Android 13+ (minSdk 33)
        // already confirms a clipboard write itself. One confirmation, the system's, instead of two.
    }

    private suspend fun openLink(item: TomiItem) {
        val url = item.url
        if (url == null || !LinkExtractor.isValidHttpUrl(url)) {
            openCard(item.id)
            return
        }
        try {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE))
        } catch (_: ActivityNotFoundException) {
            openCard(item.id)
            return
        }
        markReadQuietly(item.id)
    }

    /** The `content://` URI of the decrypted file of [slot] (the core decrypts it on demand), null when it is not available on this device. */
    private suspend fun attachmentUriOrNull(itemId: String, slot: Int): String? = try {
        container.core.attachmentUri(itemId, slot)
    } catch (e: CancellationException) {
        throw e
    } catch (_: Exception) {
        null
    }

    /**
     * "Save": every file of the item, on the explicit tap of the user, into the gallery (`Pictures/Tomi`) for an image item and into
     * `Downloads/Tomi` for the others, like the buttons of the detail do. Never for a hidden item (see [FileDownloads]). Marks the item read.
     */
    private suspend fun save(item: TomiItem) {
        if (!FileDownloads.canDownload(item)) {
            openCard(item.id)
            return
        }
        val toGallery = item.kind() == ItemKind.Image
        var saved = 0
        var unavailable = false
        var failed = false
        for (index in item.attachments.indices) {
            val result = FileDownloads.download(item, item.attachments.slotOf(index), ::attachmentUriOrNull) { uri, attachment ->
                if (toGallery) ImageExport.saveToGallery(applicationContext, uri, attachment) else saveToDownloads(applicationContext, uri, attachment)
            }
            when (result) {
                FileDownloadResult.Done -> saved++
                FileDownloadResult.Unavailable -> unavailable = true
                FileDownloadResult.Failed -> failed = true
                FileDownloadResult.Forbidden -> Unit
            }
        }
        val message = when {
            saved > 0 && !failed && !unavailable -> if (toGallery) R.string.snackbar_image_saved else R.string.snackbar_file_downloaded
            failed -> R.string.snackbar_file_download_failed
            unavailable -> R.string.snackbar_file_unavailable
            else -> null
        }
        message?.let { Toast.makeText(this, it, Toast.LENGTH_SHORT).show() }
        if (saved > 0) markReadQuietly(item.id)
    }

    /**
     * "Share": the files of the item (with its text as a caption) or its text and link go to the system share sheet, never for a hidden
     * item. The share sheet reads the files through a temporary grant: Tomi writes no copy. Marks the item read once the sheet is open.
     */
    private suspend fun share(item: TomiItem) {
        // Nothing to ask for: a hidden item never gets here (isConcealed), so the gate of the share rules is never reached.
        val noGate: suspend () -> AuthOutcome = { AuthOutcome.Failed }
        val chooserTitle = getString(R.string.share_chooser_title)
        val opened = if (FileSharing.canShare(item)) {
            when (val outcome = FileSharing.prepare(item, noGate, ::attachmentUriOrNull)) {
                is FileShareOutcome.Ready -> launchFileShare(this, outcome.files, chooserTitle, outcome.text)
                else -> {
                    Toast.makeText(this, R.string.snackbar_file_unavailable, Toast.LENGTH_SHORT).show()
                    return
                }
            }
        } else {
            when (val outcome = ItemSharing.prepare(item, noGate)) {
                is ShareOutcome.Ready -> launchSystemShare(this, outcome.payload, chooserTitle, getString(R.string.share_hidden_preview_title))
                else -> {
                    Toast.makeText(this, R.string.snackbar_nothing_to_share, Toast.LENGTH_SHORT).show()
                    return
                }
            }
        }
        if (opened) markReadQuietly(item.id) else Toast.makeText(this, R.string.share_no_app, Toast.LENGTH_SHORT).show()
    }

    private suspend fun openFile(item: TomiItem) {
        // The first attachment, by its SLOT (it is not 0 once the item lost an attachment).
        val uri = item.attachments.takeIf { it.isNotEmpty() }?.let { attachmentUriOrNull(item.id, it.slotOf(0)) }?.let(Uri::parse)
        if (uri == null) {
            openCard(item.id)
            return
        }
        val mime = item.attachments.firstOrNull()?.mime
        try {
            startActivity(
                Intent(Intent.ACTION_VIEW)
                    .setDataAndType(uri, mime)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION),
            )
        } catch (_: ActivityNotFoundException) {
            openCard(item.id)
            return
        }
        markReadQuietly(item.id)
    }

    private fun openCard(itemId: String) {
        startActivity(
            Intent(this, MainActivity::class.java)
                .setData(Uri.parse("tomi-share://item/$itemId"))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
        )
    }

    companion object {
        const val EXTRA_ITEM_ID = "eu.shaffe.tomi.extra.ITEM_ID"
        const val ACTION_TAP = "eu.shaffe.tomi.action.TAP"
        const val ACTION_COPY = "eu.shaffe.tomi.action.COPY"
        const val ACTION_VIEW_CARD = "eu.shaffe.tomi.action.VIEW_CARD"
        const val ACTION_SAVE = "eu.shaffe.tomi.action.SAVE"
        const val ACTION_SHARE = "eu.shaffe.tomi.action.SHARE"
        private const val MAX_ID_LENGTH = 64
    }
}
