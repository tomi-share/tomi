package eu.shaffe.tomi.ui.common

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipDescription
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PersistableBundle
import eu.shaffe.tomi.share.LinkExtractor
import eu.shaffe.tomi.share.SharePayload
import eu.shaffe.tomi.share.ShareActivity
import eu.shaffe.tomi.share.ShareIntentPlan

/** Opens an http(s) link in the browser. Returns false if the URL is not a plain http(s) URL or nothing can open it. */
fun openHttpLink(context: Context, url: String): Boolean {
    if (!LinkExtractor.isValidHttpUrl(url)) return false
    return try {
        context.startActivity(
            Intent(Intent.ACTION_VIEW, Uri.parse(url))
                .addCategory(Intent.CATEGORY_BROWSABLE)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        true
    } catch (_: ActivityNotFoundException) {
        false
    }
}

/**
 * Opens an attachment with the system viewer. [uri] comes from the core (`TomiCore.attachmentUri`) and must be a
 * `content://` URI of the app's own provider; anything else is refused. Returns false if it cannot be opened.
 */
fun openAttachmentUri(context: Context, uri: String, mime: String): Boolean {
    val parsed = Uri.parse(uri)
    if (parsed.scheme != "content") return false
    return try {
        context.startActivity(
            Intent(Intent.ACTION_VIEW)
                .setDataAndType(parsed, mime)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        true
    } catch (_: ActivityNotFoundException) {
        false
    } catch (_: SecurityException) {
        false
    }
}

/**
 * The `ACTION_SEND` intent for [payload], as [ShareIntentPlan] decides. For a hidden item the text also travels as a clip
 * flagged `EXTRA_IS_SENSITIVE` (hypothesis: the "Copy" button of the share sheet may then keep it out of previews and the
 * clipboard history; minSdk 33, so the flag always exists), `EXTRA_TITLE` is a neutral text, and there is no
 * `EXTRA_SUBJECT` (the title of a hidden item is not shared). `EXTRA_TITLE` does NOT replace `EXTRA_TEXT`: on Android 14+ the
 * sheet's text preview shows `EXTRA_TEXT`, so the secret may still appear there. The sheet is a system window: what it does
 * with these extras is checked on a device; the fallback is to withdraw system sharing for hidden items.
 */
fun buildShareIntent(payload: SharePayload, hiddenPreviewTitle: String): Intent {
    val plan = ShareIntentPlan.of(payload, hiddenPreviewTitle)
    val send = Intent(Intent.ACTION_SEND)
        .setType("text/plain")
        .putExtra(Intent.EXTRA_TEXT, plan.text)
    plan.subject?.let { send.putExtra(Intent.EXTRA_SUBJECT, it) }
    plan.previewTitle?.let { send.putExtra(Intent.EXTRA_TITLE, it) }
    if (plan.sensitiveClip) {
        val clip = ClipData.newPlainText(SENSITIVE_SHARE_CLIP_LABEL, plan.text)
        clip.description.extras = PersistableBundle().apply { putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true) }
        send.clipData = clip
    }
    return send
}

/** Label of the clip of a hidden item handed to the share sheet (no content in it). */
private const val SENSITIVE_SHARE_CLIP_LABEL = "Tomi"

/**
 * Opens the Android share sheet (`ACTION_SEND`, text/plain) with [payload]. Tomi's own share target is excluded from
 * the sheet, so an item is not sent back to Tomi by mistake. Returns false if no app can receive it.
 *
 * Nothing is kept: the intent only lives in this call and in the system chooser. For a hidden item this must only be
 * called after a successful authentication (see [eu.shaffe.tomi.share.ItemSharing]). [hiddenPreviewTitle] is the neutral
 * title of the sheet's preview for a hidden item (see [buildShareIntent]).
 */
fun launchSystemShare(context: Context, payload: SharePayload, chooserTitle: String, hiddenPreviewTitle: String): Boolean {
    val send = buildShareIntent(payload, hiddenPreviewTitle)
    val chooser = Intent.createChooser(send, chooserTitle)
        .putExtra(Intent.EXTRA_EXCLUDE_COMPONENTS, arrayOf(ComponentName(context, ShareActivity::class.java)))
    return try {
        context.startActivity(chooser)
        true
    } catch (_: ActivityNotFoundException) {
        false
    }
}
