package eu.shaffe.tomi.core

import uniffi.tomi_core.isInstallTokenShape as coreIsInstallTokenShape
import uniffi.tomi_core.parsePairLink
import uniffi.tomi_core.parseSetupLink

/*
 * The links and addresses that carry a relay, as the app reads them (spec/pairing-v1.md 4.3, 4.4 and 10).
 *
 * There is ONE reading, the core's (crates/core/src/relay_url.rs): the three platforms share it, so the app never re-implements what a
 * valid relay or a valid link is. It lives behind [LinkReader] so that the unit tests of the screens do not need the native library.
 *
 * A relay read here is NOT trusted: whoever shows a QR code chose it. The screens show it in full, and the core keeps nothing before the
 * pairing or the creation is confirmed.
 */

/** A pairing link. [relayUrl] is `https://host[:port]`, null for the fallback `tomi-share://pair#<code>`. [code] is a secret of the pairing. */
data class PairLink(val relayUrl: String?, val code: SensitiveText)

/** A space creation link. [token] is the installation token, a secret: it is used for one call and never logged. */
data class SetupLink(val relayUrl: String, val token: SensitiveText)

/** Reads the texts that carry a relay. Pure: no network, nothing stored, and a link never starts anything by itself. */
interface LinkReader {
    /** `https://<relay>/l/pair#<code>` or `tomi-share://pair#<code>`; null for anything else. */
    fun parsePair(text: String): PairLink?

    /** `https://<relay>/create#<token>` or `https://<relay>/create/<token>`; null for anything else. */
    fun parseSetup(text: String): SetupLink?

    /** Whether [text] has the shape of a bare installation token (the QR code of the relay administration). */
    fun isInstallTokenShape(text: String): Boolean
}

/** [LinkReader] over the functions exported by the Rust core. */
object CoreLinkReader : LinkReader {
    override fun parsePair(text: String): PairLink? = parsePairLink(text)?.let { PairLink(it.relayUrl, SensitiveText(it.code)) }

    override fun parseSetup(text: String): SetupLink? = parseSetupLink(text)?.let { SetupLink(it.relayUrl, SensitiveText(it.token)) }

    override fun isInstallTokenShape(text: String): Boolean = coreIsInstallTokenShape(text)
}

/** What the user typed in a "Relais" field, and how a relay is shown. */
object RelayAddress {
    /** Longest text looked at: a host name is at most 253 characters, plus the scheme, a port and a margin. */
    const val MAX_INPUT = 300

    /** A pairing code that is never used: it only lets the core's link reading judge a relay address (see [canonical]). */
    private const val PROBE_CODE = "4821-crayon-renard"

    /**
     * The relay typed in a field as the core reads it (`https://host[:port]`, lowercase host), or null when it is not a valid relay. The
     * scheme may be left out (`relay.example.org`). Only https is accepted, and no user information, path, query or fragment.
     *
     * The judge is the core: the text becomes a pairing link on that relay and goes through the shared reading, so the app and the core
     * can never disagree on what a valid relay is. Nothing is sent and nothing is stored.
     */
    fun canonical(raw: String, links: LinkReader): String? {
        val text = raw.trim()
        if (text.isEmpty() || text.length > MAX_INPUT || text.any { it.isISOControl() || it.isWhitespace() || it == '#' || it == '?' }) return null
        val withScheme = if (text.contains("://")) text else "https://$text"
        if (!withScheme.startsWith("https://", ignoreCase = true)) return null
        return links.parsePair("${withScheme.trimEnd('/')}/l/pair#$PROBE_CODE")?.relayUrl
    }

    /**
     * The relay a screen must use for the call it is about to make: what is typed in the "Relais" field if anything, else the relay of the
     * device, else nothing ([RelayChoice.Missing]: "Indique l'adresse du relais"). [configured] is [TomiCore.relayUrl].
     */
    fun choose(typed: String, configured: String?, links: LinkReader): RelayChoice = when {
        typed.isNotBlank() -> canonical(typed, links)?.let { RelayChoice.Typed(it) } ?: RelayChoice.Invalid
        configured.isNullOrBlank() -> RelayChoice.Missing
        else -> RelayChoice.DeviceRelay
    }

    /** Whether the "Relais" field is open and required: the device has no relay and the link (if any) names none. */
    fun fieldRequired(configured: String?, fromLink: String?): Boolean = configured.isNullOrBlank() && fromLink.isNullOrBlank()

    /** `host[:port]` of a relay URL (`https://relay.example.org:8443` gives `relay.example.org:8443`), to show in full. */
    fun authority(relayUrl: String): String = relayUrl.trim().removePrefix("https://").removePrefix("HTTPS://").substringBefore('/')
}

/** What [RelayAddress.choose] decided. */
sealed interface RelayChoice {
    /** Nothing typed: the device uses its own relay (pass no relay to the core). */
    data object DeviceRelay : RelayChoice

    /** A valid relay was typed ([url] is `https://host[:port]`): pass it to the core. */
    data class Typed(val url: String) : RelayChoice

    /** Something was typed that is not a valid relay address. */
    data object Invalid : RelayChoice

    /** No relay at all: none typed and none on the device. */
    data object Missing : RelayChoice
}
