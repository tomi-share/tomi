package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.DraftAttachment
import eu.shaffe.tomi.core.OutgoingDraft
import eu.shaffe.tomi.core.RichText
import java.time.Instant

/** A `content://` stream received through EXTRA_STREAM / ClipData, with the metadata the provider gave us. */
data class SharedStream(
    val uri: String,
    val mime: String? = null,
    val name: String? = null,
    val size: Long? = null,
) {
    override fun toString(): String = "SharedStream(<redacted>)"
}

/** Raw values of an ACTION_SEND / ACTION_SEND_MULTIPLE intent, already extracted from the Android types. */
data class ShareInput(
    val subject: String? = null,
    val text: String? = null,
    val htmlText: String? = null,
    val streams: List<SharedStream> = emptyList(),
    /** Package of the sharing app (activity referrer), when known. */
    val sourcePackage: String? = null,
    /** True if the clip carries ClipDescription.EXTRA_IS_SENSITIVE. */
    val markedSensitive: Boolean = false,
) {
    /** What another app shared is content: never printed. */
    override fun toString(): String = "ShareInput(markedSensitive=$markedSensitive, <redacted>)"
}

/** One composite item built from a share: the whole intent always becomes a single item. */
data class ParsedShare(
    val title: String?,
    val text: RichText?,
    val url: String?,
    val streams: List<SharedStream>,
    val looksSecret: Boolean,
) {
    val isEmpty: Boolean get() = title == null && text == null && url == null && streams.isEmpty()

    override fun toString(): String = "ParsedShare(looksSecret=$looksSecret, <redacted>)"
}

object SharePayloadParser {
    const val MAX_TITLE = 300
    const val MAX_TEXT = 100_000
    const val MAX_HTML = 500_000
    private const val MAX_TITLE_FROM_TEXT = 150

    fun parse(input: ShareInput, own: OwnAuthorityCheck): ParsedShare {
        val subject = TextSanitizer.clean(input.subject, MAX_TITLE)
        val html = input.htmlText?.takeIf { it.isNotBlank() }?.take(MAX_HTML)
        val rawText = TextSanitizer.clean(input.text, MAX_TEXT)
            ?: html?.let { TextSanitizer.clean(HtmlText.toPlain(it), MAX_TEXT) }

        var title = subject
        var url: String? = null
        var body = rawText

        // Chrome and most apps send "Title URL" in EXTRA_TEXT: split it into title + url.
        if (html == null && rawText != null) {
            val parts = LinkExtractor.extract(rawText)
            if (parts != null) {
                url = parts.url
                body = null
                val leftover = parts.leftover
                if (leftover != null) {
                    if (title == null) {
                        if (leftover.length <= MAX_TITLE_FROM_TEXT && '\n' !in leftover) title = leftover else body = leftover
                    } else if (!sameText(title, leftover)) {
                        body = leftover
                    }
                }
            }
        }

        val text = body?.let {
            RichText(
                plain = it,
                html = html,
                markdown = if (html == null && MarkdownDetector.looksLikeMarkdown(it)) it else null,
            )
        }

        val streams = input.streams
            .filter { StreamPolicy.isAcceptableUri(it.uri, own) }
            .take(StreamPolicy.MAX_STREAMS)

        val secret = input.markedSensitive ||
            SecretDetector.isPasswordManager(input.sourcePackage) ||
            (url == null && streams.isEmpty() && body != null && SecretDetector.looksLikeSecret(body))

        return ParsedShare(title = title, text = text, url = url, streams = streams, looksSecret = secret)
    }

    private fun sameText(a: String, b: String): Boolean =
        a.trim().equals(b.trim(), ignoreCase = true)
}

object OutgoingDraftFactory {
    /**
     * Builds the draft handed to the core. The expiry is validated against the secret flag and the attachments: a choice that is
     * not allowed (e.g. "after first read" for plain content, or for an item with a file) falls back to the default, which is no expiry.
     */
    fun create(
        id: String,
        parsed: ParsedShare,
        attachments: List<DraftAttachment>,
        secret: Boolean,
        expiry: ExpiryChoice,
        now: Instant,
    ): OutgoingDraft {
        val resolved = ExpiryPolicy.resolve(expiry, secret, hasAttachment = attachments.isNotEmpty(), now = now)
        return OutgoingDraft(
            id = id,
            title = parsed.title,
            text = parsed.text,
            url = parsed.url,
            attachments = attachments,
            secret = secret,
            expiresAt = resolved.expiresAt,
            readPolicy = resolved.readPolicy,
        )
    }
}
