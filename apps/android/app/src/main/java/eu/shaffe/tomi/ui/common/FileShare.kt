package eu.shaffe.tomi.ui.common

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import eu.shaffe.tomi.share.FileSharing
import eu.shaffe.tomi.share.ShareActivity
import eu.shaffe.tomi.share.SharedFile

/**
 * Opens the Android share sheet with the decrypted files behind [files] (`content://` of the core only). The receiving app reads
 * them through a temporary read grant: Tomi makes no copy and writes nothing to disk. The name the receiving app shows is carried by the URI
 * (the real one, except for a hidden item: `fichier.{extension}`), not by the intent.
 * [text] (description / link of the item, never for a hidden one) is the caption. Tomi's own share target is excluded. Returns false when nothing can be shared or no app can receive it.
 */
fun launchFileShare(context: Context, files: List<SharedFile>, chooserTitle: String, text: String? = null): Boolean {
    val streams = files.map { Uri.parse(it.uri) }.filter { it.scheme == "content" }
    if (streams.isEmpty() || streams.size != files.size) return false
    val send = if (streams.size == 1) {
        Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, streams.single())
    } else {
        Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(streams))
    }
    send.type = FileSharing.commonMime(files.map { it.mime })
    // The description rides along as the caption; apps that only take files ignore it.
    if (!text.isNullOrBlank()) send.putExtra(Intent.EXTRA_TEXT, text)
    send.clipData = ClipData.newRawUri("", streams.first()).also { clip -> streams.drop(1).forEach { clip.addItem(ClipData.Item(it)) } }
    send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    val chooser = Intent.createChooser(send, chooserTitle)
        .putExtra(Intent.EXTRA_EXCLUDE_COMPONENTS, arrayOf(ComponentName(context, ShareActivity::class.java)))
    return try {
        context.startActivity(chooser)
        true
    } catch (_: ActivityNotFoundException) {
        false
    }
}
