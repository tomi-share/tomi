import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/share/ImageActions.kt.

/// Result of an image action of the viewer (see `ImageActions`).
public enum ImageActionResult: Sendable, Hashable {
    /// Done: the viewer may show the image / the file was written / the share sheet was opened.
    case done
    /// The image has no readable file (index out of range, not an image, file not available, no app to receive it).
    case unavailable
    /// The file was found but writing it failed.
    case failed
    /// A hidden item and no screen lock or biometrics is set up: it stays locked.
    case biometricUnavailable
    /// A hidden item and the user cancelled or failed the authentication: nothing was decrypted, written or shared.
    case notAuthenticated
}

/// Where the image of an item may go, in one place that does not depend on UI types.
///
/// Rule (product decision, same as the text share of a hidden item, see `ItemSharing`): an image of a hidden ("Masqué") item is
/// only displayed, written to the photo library or handed to another app after `.success` of the SAME biometric gate (same
/// 2-minute window) as "Afficher". The attachment is decrypted into the core cache on demand (`resolveURI`); for a hidden
/// item that call is made only after the authentication, so a refused user triggers no decryption at all.
///
/// Nothing is kept here: the location is handed to `save` / `share` and dropped. The only copies of a hidden image are the
/// decrypted file in the core cache (purged by the core) and, on explicit request, the library entry or the file given to
/// the receiving app.
public struct ImageActions {
    /// `TomiCore.attachmentPath` (nil when unavailable). Must not throw: map errors to nil. Arguments: item id, attachment SLOT
    /// (`Attachment.slot`, NOT its position in the list: they differ once an attachment was removed from the item).
    private let resolveURI: @Sendable (_ itemId: String, _ slot: Int) async -> String?

    public init(resolveURI: @escaping @Sendable (_ itemId: String, _ slot: Int) async -> String?) {
        self.resolveURI = resolveURI
    }

    /// Authentication to open the viewer on `item`; a normal item is never asked. The viewer renders nothing for a hidden
    /// item until this returned `.done`, and only then asks for the files of its pages.
    public func authorizeView(_ item: TomiItem, authenticate: () async -> AuthOutcome) async -> ImageActionResult {
        await gate(item, authenticate)
    }

    /// File (decrypted into the core cache on demand) of one page of the viewer or of one code file, for DISPLAY. This is the
    /// rule enforced where the file is produced, not only by the screens: for a hidden item the file is requested from the core
    /// only while the authorization of `authorizeView` is still valid (`authorizationValid`: the 2-minute window of the biometric
    /// gate is open), otherwise nil and nothing is decrypted. A normal item needs no authorization. An unknown (`item` nil) or
    /// unreadable item gives nil: unknown means closed.
    public func fileForView(_ item: TomiItem?, attachmentIndex: Int, authorizationValid: () -> Bool) async -> String? {
        guard let item, item.isReadable else { return nil }
        guard item.attachments.indices.contains(attachmentIndex) else { return nil }
        if item.secret && !authorizationValid() { return nil }
        return await resolveURI(item.id, item.attachments.slot(at: attachmentIndex))
    }

    /// "Save to the photo library": `write` receives the location of the decrypted file and returns true when the entry was written.
    public func save(
        _ item: TomiItem,
        attachmentIndex: Int,
        authenticate: () async -> AuthOutcome,
        write: (_ uri: String, _ attachment: Attachment) async -> Bool
    ) async -> ImageActionResult {
        await guarded(item, attachmentIndex, authenticate) { uri, attachment in
            await write(uri, attachment) ? .done : .failed
        }
    }

    /// "Share": `launch` opens the system share sheet with the file and returns false when no app can receive it.
    public func share(
        _ item: TomiItem,
        attachmentIndex: Int,
        authenticate: () async -> AuthOutcome,
        launch: (_ uri: String, _ attachment: Attachment) async -> Bool
    ) async -> ImageActionResult {
        await guarded(item, attachmentIndex, authenticate) { uri, attachment in
            await launch(uri, attachment) ? .done : .unavailable
        }
    }

    private func guarded(
        _ item: TomiItem,
        _ attachmentIndex: Int,
        _ authenticate: () async -> AuthOutcome,
        _ action: (_ uri: String, _ attachment: Attachment) async -> ImageActionResult
    ) async -> ImageActionResult {
        guard let attachment = imageAt(item, attachmentIndex) else { return .unavailable }
        let gated = await gate(item, authenticate)
        if gated != .done { return gated }
        guard let uri = await resolveURI(item.id, item.attachments.slot(at: attachmentIndex)) else { return .unavailable }
        return await action(uri, attachment)
    }

    private func gate(_ item: TomiItem, _ authenticate: () async -> AuthOutcome) async -> ImageActionResult {
        if !item.isReadable { return .unavailable }
        if !item.secret { return .done }
        switch await authenticate() {
        case .success: return .done
        case .unavailable: return .biometricUnavailable
        case .cancelled, .failed: return .notAuthenticated
        }
    }

    private func imageAt(_ item: TomiItem, _ attachmentIndex: Int) -> Attachment? {
        guard item.attachments.indices.contains(attachmentIndex) else { return nil }
        let attachment = item.attachments[attachmentIndex]
        return attachment.isImage ? attachment : nil
    }
}
