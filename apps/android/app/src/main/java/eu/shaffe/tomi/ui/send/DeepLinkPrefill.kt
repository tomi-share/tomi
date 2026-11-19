package eu.shaffe.tomi.ui.send

import eu.shaffe.tomi.deeplink.DeepLink
import eu.shaffe.tomi.share.SharePayloadParser

/**
 * What `tomi-share://send` does to the compose fields. Any web page can open that link, so it must never throw away what the user was
 * writing (a secret included): on an empty draft the link fills the fields as before; on a non-empty one it is ADDED to the draft
 * (the title only when there is none yet) and the user is told. Nothing is ever sent from here.
 */
internal object DeepLinkPrefill {
    data class Result(val title: String, val text: String, val appended: Boolean)

    fun apply(currentTitle: String, currentText: String, hasAttachments: Boolean, link: DeepLink.Send): Result {
        val incoming = listOfNotNull(link.url, link.text).joinToString("\n")
        val draftIsEmpty = currentTitle.isBlank() && currentText.isBlank() && !hasAttachments
        if (draftIsEmpty) return Result(link.title.orEmpty(), incoming, appended = false)

        val title = if (currentTitle.isBlank()) link.title.orEmpty() else currentTitle
        val text = when {
            incoming.isEmpty() -> currentText
            currentText.isBlank() -> incoming
            else -> (currentText.trimEnd('\n') + "\n" + incoming).take(SharePayloadParser.MAX_TEXT)
        }
        return Result(title, text, appended = incoming.isNotEmpty() || (currentTitle.isBlank() && title.isNotEmpty()))
    }
}
