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
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.safeCore
import eu.shaffe.tomi.core.slotOf
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.share.LinkExtractor
import eu.shaffe.tomi.ui.render.RenderModes
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
            ACTION_COPY -> copy(item)
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

    private suspend fun openFile(item: TomiItem) {
        val uri = try {
            // The first attachment, by its SLOT (it is not 0 once the item lost an attachment).
            item.attachments.takeIf { it.isNotEmpty() }?.let { container.core.attachmentUri(item.id, it.slotOf(0)) }?.let(Uri::parse)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            null
        }
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
        private const val MAX_ID_LENGTH = 64
    }
}
