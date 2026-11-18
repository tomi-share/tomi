package eu.shaffe.tomi.ui.pairing

import eu.shaffe.tomi.core.LinkReader
import eu.shaffe.tomi.core.SensitiveText

/** What a QR code scanned on the "create your space" screen contained. Both carry a secret: nothing here prints it. */
internal sealed interface ScannedInstall {
    /** The bare installation token (the QR code of the relay administration): the relay is the one already known or typed. */
    data class Token(val token: SensitiveText) : ScannedInstall

    /** A creation link, `https://<relay>/create#<token>`: the relay (NOT trusted, shown in full before "Créer") and the token. */
    data class Link(val relayUrl: String, val token: SensitiveText) : ScannedInstall
}

/**
 * Light check of a scanned QR code before it is accepted as an installation token or a creation link. The core reads both (the single
 * reading shared by the three platforms, `LinkReader`); the relay decides whether the token is valid. This only avoids taking any QR code
 * for one.
 */
internal object InstallTokenQr {
    /** Longest bare token looked at: a token with its dashes is 39 characters, the rest is a margin for spaces. */
    private const val MAX_RAW_TOKEN = 64

    /** Longest creation link looked at: a host name (253) with its scheme, the path and a token. */
    private const val MAX_RAW_LINK = 512

    /** The scanned text read as a creation link or a bare token, or null when it is neither. Never logs the content. */
    fun parse(text: String, links: LinkReader): ScannedInstall? {
        val raw = text.trim()
        if (raw.isEmpty() || raw.length > MAX_RAW_LINK) return null
        links.parseSetup(raw)?.let { return ScannedInstall.Link(it.relayUrl, it.token) }
        if (raw.length <= MAX_RAW_TOKEN && links.isInstallTokenShape(raw)) return ScannedInstall.Token(SensitiveText(raw))
        return null
    }
}
