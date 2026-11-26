import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/core/Model.kt. Same types, same rules.
// Every type that carries clear content prints nothing of it (`description`, `debugDescription`
// and `customMirror` are redacted): logs, crash reports and test failures must not leak it.

/// Format a text was captured in (wire field `source_format`). The default display mode, never a restriction.
public enum SourceFormat: Sendable, Hashable {
    case plain, markdown, html
}

/// Text content of an item. `plain` is always present and is the safe fallback; `html` and
/// `markdown` are optional richer representations captured as-is at send time. When not given,
/// `sourceFormat` is derived from the representations that are present.
public struct RichText: Hashable, Sendable {
    public var plain: String
    public var html: String?
    public var markdown: String?
    public var sourceFormat: SourceFormat

    public init(plain: String, html: String? = nil, markdown: String? = nil, sourceFormat: SourceFormat? = nil) {
        self.plain = plain
        self.html = html
        self.markdown = markdown
        self.sourceFormat = sourceFormat ?? (markdown != nil ? .markdown : html != nil ? .html : .plain)
    }

    /// The Markdown source of the text, if it is Markdown. A sender (Windows) omits `markdown` when it equals the
    /// plain text and only declares `sourceFormat = .markdown`: the plain text then is the Markdown source.
    public var markdownSource: String? {
        markdown ?? (sourceFormat == .markdown ? plain : nil)
    }
}

/// File attached to an item. `thumbnail` is a small encoded image (PNG / JPEG) made by the
/// sender. `localPath` is set while the decrypted copy is in the core's cache; otherwise ask
/// the core (`attachmentPath`). `available` is false when the file cannot be fetched any more.
public struct Attachment: Hashable, Sendable {
    public var name: String
    public var mime: String
    public var size: Int64
    public var thumbnail: Data?
    public var thumbnailMime: String?
    public var localPath: String?
    public var available: Bool
    /// Position of the attachment in the payload of its item, STABLE for the life of the item: what the core is asked for
    /// (`TomiCore.attachmentPath`). Once an attachment was removed it is no longer its position in the list. nil (fixtures,
    /// previews): the position in the list, see `[Attachment].slot(at:)`.
    public var slot: Int?

    public init(
        name: String, mime: String, size: Int64, thumbnail: Data? = nil, thumbnailMime: String? = nil,
        localPath: String? = nil, available: Bool = true, slot: Int? = nil
    ) {
        self.name = name
        self.mime = mime
        self.size = size
        self.thumbnail = thumbnail
        self.thumbnailMime = thumbnailMime
        self.localPath = localPath
        self.available = available
        self.slot = slot
    }

    public var isImage: Bool { mime.hasPrefix("image/") }
}

extension Array where Element == Attachment {
    /// Slot of the attachment at `index` of the list: its own, or (fixtures) its position.
    public func slot(at index: Int) -> Int { self[index].slot ?? index }
}

public enum ReadPolicy: Sendable, Hashable {
    case normal, afterFirstRead
}

public enum DeliveryState: Sendable, Hashable {
    /// Stored locally, not yet accepted by the relay (offline, or still inside the 3 s undo window).
    case queued
    /// Accepted by the relay, not yet fetched by that device.
    case sent
    /// Fetched by that device.
    case delivered
    /// The item expired before that device could fetch it.
    case expiredBeforeDelivery
    case failed
}

public struct DeviceRef: Hashable, Sendable {
    public var id: String
    public var name: String

    public init(id: String, name: String) {
        self.id = id
        self.name = name
    }
}

/// Delivery status of an outgoing item for one recipient device.
public struct DeviceDelivery: Hashable, Sendable {
    public var deviceId: String
    public var deviceName: String
    public var state: DeliveryState

    public init(deviceId: String, deviceName: String, state: DeliveryState) {
        self.deviceId = deviceId
        self.deviceName = deviceName
        self.state = state
    }
}

/// Why an item received from another device cannot be shown. The item is never dropped because of
/// it (DEV.md invariant 7).
public enum UnreadableReason: Sendable, Hashable {
    /// The item uses a protocol or content version this app does not know yet.
    case updateRequired
    /// The item could not be decrypted or decoded. It is kept and retried after a sync or an update.
    case decryptionFailed
    /// Sealed with a key this device does not have yet (retried after the next rotation).
    case keyPending
    /// Sealed with a replaced key after the rotation that replaced it: not displayed.
    case suspect
}

/// Composite content: every field is optional.
public struct ReadableContent: Hashable, Sendable {
    public var title: String?
    public var text: RichText?
    public var url: String?
    public var attachments: [Attachment]
    public var secret: Bool

    public init(title: String? = nil, text: RichText? = nil, url: String? = nil, attachments: [Attachment] = [], secret: Bool = false) {
        self.title = title
        self.text = text
        self.url = url
        self.attachments = attachments
        self.secret = secret
    }
}

/// What an item contains, or the reason why it cannot be shown.
public enum ItemContent: Hashable, Sendable {
    case readable(ReadableContent)
    /// The core holds the item but cannot expose its content.
    case unreadable(UnreadableReason)
}

/// One item of the feed: metadata that is always known, plus `content`. The shortcuts return
/// empty values for an unreadable item.
public struct TomiItem: Identifiable, Hashable, Sendable {
    public var id: String
    public var content: ItemContent
    public var sourceDevice: DeviceRef
    public var createdAt: Date
    public var expiresAt: Date?
    public var readPolicy: ReadPolicy
    public var read: Bool
    public var deliveries: [DeviceDelivery]
    /// Pinned on all devices (spec/pin-contract.md).
    public var pinned: Bool
    /// Time of the decision to pin, to sort the Pinned section (most recent first).
    public var pinnedAt: Date?
    /// Archived: out of "Recent", still readable and searchable.
    /// Never set together with `pinned` by the core; if both ever come, the UI shows it pinned.
    public var archived: Bool
    public var archivedAt: Date?

    public init(
        id: String, content: ItemContent, sourceDevice: DeviceRef, createdAt: Date, expiresAt: Date? = nil,
        readPolicy: ReadPolicy = .normal, read: Bool = false, deliveries: [DeviceDelivery] = [],
        pinned: Bool = false, pinnedAt: Date? = nil, archived: Bool = false, archivedAt: Date? = nil
    ) {
        self.id = id
        self.content = content
        self.sourceDevice = sourceDevice
        self.createdAt = createdAt
        self.expiresAt = expiresAt
        self.readPolicy = readPolicy
        self.read = read
        self.deliveries = deliveries
        self.pinned = pinned
        self.pinnedAt = pinnedAt
        self.archived = archived
        self.archivedAt = archivedAt
    }

    /// Builds a readable item without spelling out `ItemContent.readable` (Kotlin `readableItem`).
    public static func readable(
        id: String, sourceDevice: DeviceRef, createdAt: Date, title: String? = nil, text: RichText? = nil,
        url: String? = nil, attachments: [Attachment] = [], secret: Bool = false, expiresAt: Date? = nil,
        readPolicy: ReadPolicy = .normal, read: Bool = false, deliveries: [DeviceDelivery] = [],
        pinned: Bool = false, pinnedAt: Date? = nil, archived: Bool = false, archivedAt: Date? = nil
    ) -> TomiItem {
        TomiItem(
            id: id,
            content: .readable(ReadableContent(title: title, text: text, url: url, attachments: attachments, secret: secret)),
            sourceDevice: sourceDevice, createdAt: createdAt, expiresAt: expiresAt, readPolicy: readPolicy,
            read: read, deliveries: deliveries, pinned: pinned, pinnedAt: pinnedAt, archived: archived,
            archivedAt: archivedAt
        )
    }

    private var readableContent: ReadableContent? {
        if case let .readable(content) = content { return content }
        return nil
    }

    public var title: String? { readableContent?.title }
    public var text: RichText? { readableContent?.text }
    public var url: String? { readableContent?.url }
    public var attachments: [Attachment] { readableContent?.attachments ?? [] }
    public var secret: Bool { readableContent?.secret ?? false }
    public var isReadable: Bool { readableContent != nil }

    public func isOutgoing(localDeviceId: String) -> Bool { sourceDevice.id == localDeviceId }

    /// Whether merely opening the card marks the item read. Never for an item already read or sent
    /// from this device, a secret (read only once revealed or copied), or an unreadable item (the
    /// user did not see it, and an "after first read" item could become readable after an update).
    public func shouldMarkReadWhenOpened(localDeviceId: String) -> Bool {
        !read && !secret && isReadable && !isOutgoing(localDeviceId: localDeviceId)
    }

    /// Visual and behavioural category. An unreadable item or a secret always wins over the rest.
    public var kind: ItemKind {
        if !isReadable { return .unreadable }
        if secret { return .secret }
        if !attachments.isEmpty && attachments.allSatisfy(\.isImage) { return .image }
        if !attachments.isEmpty { return .file }
        if url != nil { return .link }
        if let text, text.html != nil || text.markdown != nil || text.sourceFormat != .plain { return .richText }
        return .text
    }
}

public enum ItemKind: Sendable, Hashable {
    case unreadable, secret, image, file, link, richText, text
}

public enum DevicePlatform: Sendable, Hashable {
    case android, windows, ios, macos, cli, other

    /// From the core's platform string (`android`, `windows`, `ios`, `macos`, `cli`, anything else).
    public init(core value: String) {
        switch value {
        case "android": self = .android
        case "windows": self = .windows
        case "ios": self = .ios
        case "macos": self = .macos
        case "cli": self = .cli
        default: self = .other
        }
    }
}

public struct Device: Identifiable, Hashable, Sendable {
    public var id: String
    public var name: String
    public var platform: DevicePlatform
    public var lastActivity: Date?
    public var isThisDevice: Bool
    /// The core checked the certificate of this device against the chain of the space. `false` for a device that the relay's
    /// log lists without a certificate the space accepts: it shows in the list (it may be revoked), but is never announced.
    public var isVerified: Bool

    public init(
        id: String, name: String, platform: DevicePlatform, lastActivity: Date?, isThisDevice: Bool,
        isVerified: Bool = true
    ) {
        self.id = id
        self.name = name
        self.platform = platform
        self.lastActivity = lastActivity
        self.isThisDevice = isThisDevice
        self.isVerified = isVerified
    }
}

/// An attachment staged on disk by the app; the core reads it before `enqueue` returns.
public struct DraftAttachment: Hashable, Sendable {
    public var path: String
    public var name: String
    public var mime: String
    public var size: Int64
    /// Preview made by the sending app (images, and what Quick Look can draw), sent inside the item.
    public var thumbnail: Data?
    public var thumbnailMime: String?

    public init(path: String, name: String, mime: String, size: Int64, thumbnail: Data? = nil, thumbnailMime: String? = nil) {
        self.path = path
        self.name = name
        self.mime = mime
        self.size = size
        self.thumbnail = thumbnail
        self.thumbnailMime = thumbnailMime
    }
}

/// What the UI hands to the core. Creation time and delivery state are assigned by the core.
public struct OutgoingDraft: Hashable, Sendable {
    public var id: String
    public var title: String?
    public var text: RichText?
    public var url: String?
    public var attachments: [DraftAttachment]
    public var secret: Bool
    public var expiresAt: Date?
    public var readPolicy: ReadPolicy

    public init(
        id: String, title: String? = nil, text: RichText? = nil, url: String? = nil, attachments: [DraftAttachment] = [],
        secret: Bool = false, expiresAt: Date? = nil, readPolicy: ReadPolicy = .normal
    ) {
        self.id = id
        self.title = title
        self.text = text
        self.url = url
        self.attachments = attachments
        self.secret = secret
        self.expiresAt = expiresAt
        self.readPolicy = readPolicy
    }
}

/// What `TomiCore.deleteAttachment` did.
public enum AttachmentRemoval: Sendable, Hashable {
    /// The attachment is gone (or already was); the item stays.
    case attachmentRemoved
    /// It was the last thing in the item (no other attachment, no text, no URL): the whole item was deleted instead, with
    /// the same scope.
    case itemDeleted
}

public enum DispatchResult: Sendable, Hashable {
    /// The relay accepted the item.
    case sent
    /// No network or a relay error: retry later.
    case retryLater
    /// The item no longer exists locally (cancelled, deleted): nothing to do.
    case notFound
    /// Refused for good by the relay, or expired before sending.
    case failed
}

/// State of the space this device belongs to.
public enum SpaceState: Sendable, Hashable {
    case active
    /// This device was removed from the space: nothing can be sent or read any more.
    case revoked
    /// The space uses a protocol version newer than this app.
    case updateRequired
    /// Blocked for another reason (format limited by another device, space deleted...).
    case blocked
    /// The relay does not find the space or this device ("espace introuvable"). Not a proof of anything: nothing is
    /// erased.
    case notFound
}

/// Why a pairing or a creation of the space failed (Android `core/Pairing.kt` `PairingFailure`).
public enum PairingFailure: Error, Sendable, Hashable, CaseIterable {
    /// The code or link cannot be read: nothing was consumed, fix the typo.
    case invalidCode
    /// The verification failed (the codes would differ): generate a new code.
    case wrongCode
    case expired
    case network
    /// Another device already used this code.
    case taken
    /// The connection dropped after the code was claimed: the code is spent.
    case interrupted
    /// The other device cancelled.
    case cancelled
    case updateRequired
    case spaceFull
    /// Two pairings are already open in the space.
    case anotherPairingOpen
    case installTokenInvalid
    /// The space already exists: join it instead.
    case spaceExists
    case blocked
    case unexpected
    /// The demo core's simulated pairing refuses (it answers only its own demo codes).
    case unsupported
    /// No relay at all: the build has none, and neither a link nor the "Relais" field gave one. Nothing was sent.
    case relayNotConfigured
    /// The relay of the link or of the field is not a valid relay address (`https` only...). Nothing was sent.
    case invalidRelay
}

/// The other device of a pairing, as the verification screen shows it.
public struct PairingPeer: Sendable, Hashable {
    public let name: String
    public let platform: DevicePlatform

    public init(name: String, platform: DevicePlatform) {
        self.name = name
        self.platform = platform
    }
}

/// The other device was reached: the user compares `verificationCode` (6 digits) on both screens.
public struct PairingPrompt: Sendable, Hashable {
    public let peer: PairingPeer
    /// The 6 digits, without separator.
    public let verificationCode: String
    /// Items this device could import (0: nothing to ask).
    public let historyItemCount: Int
    /// Bytes of that history, an indication (0: unknown).
    public let historyBytes: Int64
    /// Devices of the space before this one joins, if known.
    public let memberCount: Int?
    /// Validity of the code from the answer, in milliseconds: an indication, never a cut-off on this side.
    public let expiresInMs: Int64
    /// Authority (`host[:port]`) of the relay of this pairing, to show in full next to the verification code: it may
    /// come from a QR code or a field, so the user reads and confirms it (spec/pairing-v1.md 4.4 rule 7).
    public let relayAuthority: String

    public init(
        peer: PairingPeer, verificationCode: String, historyItemCount: Int, historyBytes: Int64 = 0,
        memberCount: Int? = nil, expiresInMs: Int64 = 120_000, relayAuthority: String = ""
    ) {
        self.peer = peer
        self.verificationCode = verificationCode
        self.historyItemCount = historyItemCount
        self.historyBytes = historyBytes
        self.memberCount = memberCount
        self.expiresInMs = expiresInMs
        self.relayAuthority = relayAuthority
    }
}

public enum PairingResult: Sendable, Hashable {
    /// This device has NOT joined yet: the user compares the codes, then the app confirms or cancels.
    case awaitingVerification(PairingPrompt)
    case failure(PairingFailure)
    /// This device is already in the space (a pairing interrupted by a stop was settled meanwhile).
    case alreadyPaired
}

/// What the user of the inviting device compares and decides on.
public struct InvitePrompt: Sendable, Hashable {
    /// The new device, as it announced itself.
    public let peer: PairingPeer
    /// The 6 digits, without separator.
    public let verificationCode: String

    public init(peer: PairingPeer, verificationCode: String) {
        self.peer = peer
        self.verificationCode = verificationCode
    }
}

/// The new device, registered in the space. `historyAnnounced`: it asked for the history, whose export starts at the
/// next sync.
public struct InvitedDevice: Sendable, Hashable {
    public let deviceId: String
    public let name: String
    public let platform: DevicePlatform
    public let historyAnnounced: Bool

    public init(deviceId: String, name: String, platform: DevicePlatform, historyAnnounced: Bool) {
        self.deviceId = deviceId
        self.name = name
        self.platform = platform
        self.historyAnnounced = historyAnnounced
    }
}

/// One invitation, from the code to the registered device (Android `InviteSession`): this device is a member and adds
/// a new one. The core drives nothing by itself: the calls below are the driver.
///
///     code / link on screen → awaitPeer() → verification code and new device on screen
///       → [user: "Ajouter"] confirm() → awaitDone()          [user: "Annuler" or leaves: cancel()]
///
/// The waiting calls take minutes at most; `cancel` from anywhere ends them. Failures are `PairingFailure`.
public protocol InviteSession: AnyObject, Sendable {
    /// The code to show (`4821-crayon-renard`); nil once the session is over. Never copied anywhere.
    var code: String? { get }
    /// The link of the QR code, drawn exactly as it is; nil once the session is over.
    var link: String? { get }
    /// Milliseconds left before the relay drops an unused code (never negative).
    func expiresInMs() -> Int64
    /// Until the new device said hello. `.expired` when the code ran out.
    func awaitPeer() async throws -> InvitePrompt
    /// The user compared the codes and pressed "Ajouter": nothing is given yet, both people must confirm.
    func confirm() async throws
    /// Until the new device is part of the space.
    func awaitDone() async throws -> InvitedDevice
    /// Closes the channel at the relay and wipes the session. Any time, more than once.
    func cancel() async
}

/// What settling an interrupted pairing at start-up did (Android `ResumeOutcome`).
public enum ResumeOutcome: Sendable, Hashable {
    case nothingPending, provisioned, discarded
    /// The relay could not be reached after a few tries: the next start tries again.
    case unreachable
}

/// What the demo core can make another device send (debug menus).
public enum SimulatedIncoming: Sendable, Hashable, CaseIterable {
    case link, text, richText, image, file, secret, unreadable
}

/// Error of a core call. Never carries content or keys (the core guarantees it).
public struct TomiCoreError: Error, Hashable, Sendable, CustomStringConvertible {
    public enum Kind: Hashable, Sendable {
        case storage, network, blocked, invalidInput, `internal`
        case relay(status: Int, code: String)
        case crypto(code: String)
    }

    public var kind: Kind
    public var reason: String

    public init(_ kind: Kind, reason: String = "") {
        self.kind = kind
        self.reason = reason
    }

    public var description: String { "TomiCoreError(\(kind), \(reason))" }
}

// MARK: - Redaction

private func redactedMirror(_ subject: Any) -> Mirror {
    Mirror(subject, children: [], displayStyle: .struct)
}

extension RichText: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "RichText(<redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { redactedMirror(self) }
}

extension Attachment: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "Attachment(<redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { redactedMirror(self) }
}

extension ReadableContent: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "Readable(secret=\(secret), attachments=\(attachments.count), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { redactedMirror(self) }
}

extension ItemContent: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String {
        switch self {
        case let .readable(content): content.description
        case let .unreadable(reason): "Unreadable(\(reason))"
        }
    }

    public var debugDescription: String { description }
    public var customMirror: Mirror { redactedMirror(self) }
}

extension TomiItem: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "TomiItem(id=\(id), secret=\(secret), readable=\(isReadable), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { redactedMirror(self) }
}

extension DraftAttachment: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "DraftAttachment(<redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { redactedMirror(self) }
}

extension OutgoingDraft: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "OutgoingDraft(id=\(id), secret=\(secret), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { redactedMirror(self) }
}
