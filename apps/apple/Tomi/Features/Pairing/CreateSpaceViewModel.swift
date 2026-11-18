import Foundation
import Observation
import TomiKit

/// "Crée ton espace" (Android `CreateSpaceViewModel`): the first device of a space has nobody to ask for a code, so it
/// creates the space with the installation token the relay printed at its start (spec/pairing-v1.md 10). The token is
/// a secret: typed, pasted or scanned, handed to the core for this one call, and dropped as soon as the call succeeds
/// or the screen is left (`clear`).
///
/// The relay comes from the creation link (`https://<relay>/create#<token>`, which also carries the token), from the
/// "Relais" field, or from the device. A relay that was read is not trusted: the screen shows it in full before the
/// token leaves the device, and the core keeps nothing before the space is created.
@MainActor
@Observable
final class CreateSpaceViewModel {
    private(set) var token = ""
    private(set) var deviceName: String
    private(set) var busy = false
    /// Error to show (a text key).
    private(set) var error: String?
    /// The space already exists: "Rejoindre l'espace existant" is offered.
    private(set) var spaceExists = false
    private(set) var done = false
    /// What is in the "Relais" field. A creation link or a scanned link fills it; the user can correct it.
    private(set) var relayText = ""
    /// The field was filled by a link or a QR code (not typed): said below the relay.
    private(set) var relayFromLink = false
    /// URL of the relay the device already has, nil when it has none (then the field is required).
    private(set) var deviceRelay: String?

    private let core: any TomiCore
    private let reader: any LinkReading
    static let maxNameInput = 64

    /// `suggestedName`: the name of the phone, cleaned; the core's rule decides, "iPhone" when nothing fits.
    init(core: any TomiCore, suggestedName: String?, reader: any LinkReading = DeepLinkParser.reader) {
        self.core = core
        self.reader = reader
        deviceName = DeviceNames.choose([suggestedName], isValid: core.isValidDeviceName)
    }

    // MARK: Relay

    /// The relay the creation will use, from the field and the device: usable, "missing" or "invalid".
    var relayChoice: RelayAddress.Choice {
        RelayAddress.choose(field: relayText, deviceRelay: deviceRelay, reader: reader)
    }

    /// The field is open and required: the device has no relay of its own.
    var relayFieldRequired: Bool { deviceRelay == nil }

    /// Authority of the relay the device already has, to name it above the folded field.
    var deviceRelayAuthority: String? { deviceRelay.flatMap(RelayAddress.authority(of:)) }

    func onRelayChange(_ value: String) {
        relayText = String(value.prefix(RelayAddress.maxInput))
        relayFromLink = false
        error = nil
    }

    /// Reads the relay of the device (when the screen shows).
    func loadDeviceRelay() async {
        deviceRelay = await core.currentRelay()
    }

    /// A creation link opened the app: it fills the token and the relay, nothing is submitted.
    func offerLink(relayUrl: String, token: String) {
        apply(relayUrl: relayUrl, token: token)
    }

    // MARK: Input

    /// The token field: a bare token, or a pasted creation link, which fills the relay too.
    func onTokenChange(_ value: String) {
        let raw = String(value.prefix(CreateInput.maxRaw))
        if case let .link(relayUrl, linkToken)? = CreateInput.read(raw, reader: reader) {
            apply(relayUrl: relayUrl, token: linkToken)
            return
        }
        token = String(raw.prefix(InstallTokenQr.maxInput))
        error = nil
        spaceExists = false
    }

    private func apply(relayUrl: String, token: String) {
        relayText = relayUrl
        relayFromLink = true
        self.token = String(token.prefix(InstallTokenQr.maxInput))
        error = nil
        spaceExists = false
    }

    func onDeviceNameChange(_ value: String) {
        // 64 UTF-16 units at most, as Android; never half a character.
        var kept = ""
        for character in value {
            guard kept.utf16.count + String(character).utf16.count <= Self.maxNameInput else { break }
            kept.append(character)
        }
        deviceName = kept
        error = nil
    }

    /// A scanned QR code fills the field when it is a creation link or looks like a token; nothing is submitted.
    func onQRScanned(_ text: String) {
        guard !busy else { return }
        switch CreateInput.read(text, reader: reader) {
        case let .link(relayUrl, linkToken)?:
            apply(relayUrl: relayUrl, token: linkToken)
        case let .token(scanned)?:
            onTokenChange(scanned)
        case nil:
            error = "create_error_qr"
        }
    }

    func submit() async {
        guard !busy else { return }
        let name = deviceName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard core.isValidDeviceName(name) else {
            error = "create_error_name"
            return
        }
        let typed = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !typed.isEmpty else {
            error = "create_error_token_empty"
            return
        }
        // The relay that gets the token is settled, and was shown, before anything is sent.
        let choice = relayChoice
        if let key = choice.errorKey {
            error = key
            return
        }
        busy = true
        error = nil
        spaceExists = false
        defer { busy = false }
        do {
            try await core.createSpace(installToken: typed, deviceName: name, relayUrl: choice.urlToPass)
            token = ""
            relayText = ""
            done = true
        } catch {
            let reason = error as? PairingFailure ?? .unexpected
            self.error = PairingErrors.createMessageKey(reason)
            spaceExists = reason == .spaceExists
            // The relay the core had pointed at is back.
            await loadDeviceRelay()
        }
    }

    /// The screen is left: the token goes with it.
    func clear() {
        token = ""
    }
}
