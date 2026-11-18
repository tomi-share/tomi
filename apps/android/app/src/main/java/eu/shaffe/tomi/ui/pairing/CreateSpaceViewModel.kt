package eu.shaffe.tomi.ui.pairing

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.PairingException
import eu.shaffe.tomi.core.PairingFailure
import eu.shaffe.tomi.core.RelayAddress
import eu.shaffe.tomi.core.RelayChoice
import eu.shaffe.tomi.core.SensitiveText
import eu.shaffe.tomi.core.TomiCoreException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** The message of a failure of the creation of the space. */
internal fun createFailureMessage(reason: PairingFailure): Int = when (reason) {
    PairingFailure.InstallTokenInvalid -> R.string.create_error_token
    PairingFailure.SpaceExists -> R.string.create_error_space_exists
    PairingFailure.Network, PairingFailure.Interrupted -> R.string.pair_error_network
    PairingFailure.UpdateRequired -> R.string.pair_error_update
    PairingFailure.Blocked -> R.string.pair_error_blocked
    PairingFailure.RelayNotConfigured -> R.string.pair_error_relay_required
    PairingFailure.InvalidRelay -> R.string.pair_error_relay_invalid
    PairingFailure.InvalidCode, PairingFailure.WrongCode, PairingFailure.Expired, PairingFailure.Taken, PairingFailure.Cancelled,
    PairingFailure.SpaceFull, PairingFailure.AnotherPairingOpen, PairingFailure.Unexpected,
    -> R.string.pair_error_unexpected
}

/**
 * "Create your space": the first device of a space has nobody to ask for a code, so it creates the space with the installation token
 * that the relay printed at its start (spec/pairing-v1.md 10). The token is a secret: it is typed or pasted, handed to the core for
 * this one call, and dropped from this screen as soon as the call succeeds or the screen is left.
 *
 * Scoped to its screen (the navigation entry).
 */
class CreateSpaceViewModel(private val container: AppContainer) : ViewModel() {
    var token by mutableStateOf("")
        private set
    var deviceName by mutableStateOf(container.defaultDeviceName())
        private set

    /** The relay typed in the "Relais" field, or read from a creation link (`https://host[:port]`). NOT trusted: shown in full before "Créer". */
    var relayText by mutableStateOf("")
        private set

    /** The relay of the device (`TomiCore.relayUrl`), null in a release build that has none yet; read once at start. */
    var configuredRelay by mutableStateOf<String?>(null)
        private set

    /** The relay was read from a link or a QR code (not typed): the screen says so. Typing in the field makes it a typed one. */
    var relayFromLink by mutableStateOf(false)
        private set

    /** The field is open and required: no relay on the device and none read from a link. */
    val relayRequired: Boolean get() = RelayAddress.fieldRequired(configuredRelay, relayText.takeIf { relayFromLink })

    /** The relay that "Créer" will use, as `host[:port]` to show in full; null when none is known yet or what is typed is not a relay. */
    var relayAuthority by mutableStateOf<String?>(null)
        private set

    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<Int?>(null)
        private set

    /** The relay already has a space: the user should join it instead. */
    var spaceExists by mutableStateOf(false)
        private set

    /** The space is created and this device is in it. */
    var done by mutableStateOf(false)
        private set

    init {
        viewModelScope.launch {
            try {
                configuredRelay = container.core.relayUrl()
            } catch (e: CancellationException) {
                throw e
            } catch (_: TomiCoreException) {
                // No relay known: the field stays required.
            }
            refreshRelayAuthority()
        }
    }

    private fun refreshRelayAuthority() {
        relayAuthority = when (val choice = RelayAddress.choose(relayText, configuredRelay, container.links)) {
            RelayChoice.DeviceRelay -> configuredRelay?.let(RelayAddress::authority)
            is RelayChoice.Typed -> RelayAddress.authority(choice.url)
            RelayChoice.Invalid, RelayChoice.Missing -> null
        }
    }

    fun onRelayChange(value: String) {
        relayText = value.take(RelayAddress.MAX_INPUT)
        relayFromLink = false
        error = null
        refreshRelayAuthority()
    }

    /**
     * A creation link arrived (App Link, or a QR code scanned in the app): the relay and the token fill the form and NOTHING is submitted,
     * the user checks the relay and presses "Créer mon espace".
     */
    fun onSetupLink(relayUrl: String, token: SensitiveText) {
        if (busy) return
        onTokenChange(token.reveal())
        relayText = relayUrl
        relayFromLink = true
        refreshRelayAuthority()
    }

    fun onTokenChange(value: String) {
        token = value.take(MAX_TOKEN)
        error = null
        spaceExists = false
    }

    /**
     * A QR code scanned on the "create your space" screen. It is NOT a pairing link: it is a creation link (`https://<relay>/create#<token>`,
     * the relay and the token) or the bare installation token (the QR of the relay administration; the relay is then the known or typed
     * one). A content that is neither is refused with a message and the form is left alone. A valid one goes through [onTokenChange] (same
     * life cycle as a typed token) and nothing is submitted: the user still taps "Create my space".
     */
    fun onQrScanned(text: String) {
        if (busy) return
        when (val scanned = InstallTokenQr.parse(text, container.links)) {
            null -> error = R.string.create_error_qr
            is ScannedInstall.Token -> onTokenChange(scanned.token.reveal())
            is ScannedInstall.Link -> onSetupLink(scanned.relayUrl, scanned.token)
        }
    }

    fun onDeviceNameChange(value: String) {
        deviceName = value.take(MAX_NAME)
        error = null
    }

    fun submit() {
        if (busy) return
        val name = deviceName.trim()
        if (!container.core.isValidDeviceName(name)) {
            error = R.string.create_error_name
            return
        }
        val secret = SensitiveText(token.trim())
        if (secret.isBlank) {
            error = R.string.create_error_token_empty
            return
        }
        // The relay is settled before anything is sent: the one typed or read from the link, else the one of the device.
        val relay = when (val choice = RelayAddress.choose(relayText, configuredRelay, container.links)) {
            RelayChoice.DeviceRelay -> null
            is RelayChoice.Typed -> choice.url
            RelayChoice.Invalid -> {
                error = R.string.pair_error_relay_invalid
                return
            }
            RelayChoice.Missing -> {
                error = R.string.pair_error_relay_required
                return
            }
        }
        busy = true
        error = null
        spaceExists = false
        viewModelScope.launch {
            try {
                container.core.createSpace(secret, name, relay)
                // Used once: it is not kept.
                token = ""
                done = true
            } catch (e: CancellationException) {
                throw e
            } catch (e: TomiCoreException) {
                val reason = (e as? PairingException)?.reason ?: PairingFailure.Unexpected
                error = createFailureMessage(reason)
                spaceExists = reason == PairingFailure.SpaceExists
            } finally {
                busy = false
            }
        }
    }

    override fun onCleared() {
        token = ""
    }

    private companion object {
        // The token of the relay is 8 groups of 4 characters (api-v1.md 3.1); a margin for the way it is pasted.
        const val MAX_TOKEN = 128
        const val MAX_NAME = 64
    }
}
