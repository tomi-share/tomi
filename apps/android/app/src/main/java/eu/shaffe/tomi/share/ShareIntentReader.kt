package eu.shaffe.tomi.share

import android.app.Activity
import android.content.ClipDescription
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.content.IntentCompat
import eu.shaffe.tomi.send.readStreamMetadata
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** What the intent carries, before any content provider has been asked anything. */
class RawShare(
    val subject: String?,
    val text: String?,
    val htmlText: String?,
    val uris: List<Uri>,
    val sourcePackage: String?,
    val markedSensitive: Boolean,
)

/** Extracts the raw share values from an ACTION_SEND / ACTION_SEND_MULTIPLE intent. No decision is made here. */
object ShareIntentReader {

    /** Reads the extras only (no provider call): cheap, and the only part that needs the activity. Main thread. */
    fun extract(activity: Activity, intent: Intent): RawShare {
        val multiple = intent.action == Intent.ACTION_SEND_MULTIPLE

        val text: String? = if (multiple) {
            runCatching { intent.getCharSequenceArrayListExtra(Intent.EXTRA_TEXT) }.getOrNull()
                ?.joinToString("\n") { it.toString() }
        } else {
            runCatching { intent.getCharSequenceExtra(Intent.EXTRA_TEXT) }.getOrNull()?.toString()
        }

        val uris = LinkedHashSet<Uri>()
        if (multiple) {
            IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.let(uris::addAll)
        } else {
            IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.let(uris::add)
        }
        if (uris.isEmpty()) {
            intent.clipData?.let { clip ->
                for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let(uris::add)
            }
        }

        return RawShare(
            subject = runCatching { intent.getStringExtra(Intent.EXTRA_SUBJECT) }.getOrNull(),
            text = text,
            htmlText = runCatching { intent.getStringExtra(Intent.EXTRA_HTML_TEXT) }.getOrNull(),
            uris = uris.toList(),
            sourcePackage = activity.referrer?.takeIf { it.scheme == "android-app" }?.host,
            markedSensitive = intent.clipData?.description?.extras?.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE, false) == true,
        )
    }

    /**
     * Drops the URIs we must not read (our own, `file://`) and asks each remaining provider for its name, size and
     * type: up to [StreamPolicy.MAX_STREAMS] provider queries, which can be slow, so this runs on the IO dispatcher
     * and must never be called from the main thread.
     */
    suspend fun resolve(context: Context, raw: RawShare, own: OwnAuthorityCheck): ShareInput = withContext(Dispatchers.IO) {
        val streams = raw.uris.asSequence()
            .filter { StreamPolicy.isAcceptable(it.scheme, it.authority, own) }
            .take(StreamPolicy.MAX_STREAMS)
            .map { readStreamMetadata(context, it) }
            .toList()
        ShareInput(
            subject = raw.subject,
            text = raw.text,
            htmlText = raw.htmlText,
            streams = streams,
            sourcePackage = raw.sourcePackage,
            markedSensitive = raw.markedSensitive,
        )
    }
}
