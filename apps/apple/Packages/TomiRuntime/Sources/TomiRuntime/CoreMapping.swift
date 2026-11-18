import Foundation
import TomiCore
import TomiKit

/// Conversions between the UniFFI types of the core and the app model (`TomiKit`).
enum CoreMapping {
    static func date(_ ms: Int64) -> Date {
        Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
    }

    static func ms(_ date: Date) -> Int64 {
        Int64((date.timeIntervalSince1970 * 1000).rounded())
    }

    static func item(_ f: FfiItem) -> TomiItem {
        let content: ItemContent
        switch f.content {
        case let .readable(title, text, url, attachments, secret):
            content = .readable(ReadableContent(
                title: title,
                text: text.map(richText),
                url: url,
                attachments: attachments.map(attachment),
                secret: secret
            ))
        case let .unreadable(reason):
            content = .unreadable(unreadable(reason))
        }
        return TomiItem(
            id: f.id,
            content: content,
            sourceDevice: DeviceRef(id: f.sourceDeviceId, name: f.sourceDeviceName),
            createdAt: date(f.createdAtMs),
            expiresAt: f.expiresAtMs.map(date),
            readPolicy: f.readPolicy == .afterFirstRead ? .afterFirstRead : .normal,
            read: f.read,
            deliveries: f.deliveries.map {
                DeviceDelivery(deviceId: $0.deviceId, deviceName: $0.deviceName, state: delivery($0.state))
            },
            pinned: f.pinned,
            pinnedAt: f.pinnedAtMs.map(date),
            archived: f.archived && !f.pinned,
            archivedAt: f.archived && !f.pinned ? f.archivedAtMs.map(date) : nil
        )
    }

    static func richText(_ t: TextContent) -> RichText {
        RichText(plain: t.plain, html: t.html, markdown: t.markdown, sourceFormat: sourceFormat(t.sourceFormat))
    }

    static func sourceFormat(_ f: TextFormat) -> SourceFormat {
        switch f {
        case .plain: .plain
        case .markdown: .markdown
        case .html: .html
        }
    }

    static func textFormat(_ f: SourceFormat) -> TextFormat {
        switch f {
        case .plain: .plain
        case .markdown: .markdown
        case .html: .html
        }
    }

    static func attachment(_ a: FfiAttachment) -> Attachment {
        Attachment(
            name: a.name,
            mime: a.mime,
            size: Int64(clamping: a.size),
            thumbnail: a.thumbnail,
            thumbnailMime: a.thumbnailMime,
            localPath: a.localPath,
            available: a.available,
            slot: Int(clamping: a.slot)
        )
    }

    static func unreadable(_ r: UnreadableKind) -> UnreadableReason {
        switch r {
        case .updateRequired: .updateRequired
        case .decryptionFailed: .decryptionFailed
        case .keyPending: .keyPending
        case .suspect: .suspect
        }
    }

    static func delivery(_ d: DeliveryKind) -> DeliveryState {
        switch d {
        case .queued: .queued
        case .sent: .sent
        case .delivered: .delivered
        case .expiredBeforeDelivery: .expiredBeforeDelivery
        case .failed: .failed
        }
    }

    /// Revoked devices are not part of the app model (the Devices screen never lists them),
    /// except this device itself (`keepRevoked`), which must stay known.
    static func device(_ d: FfiDevice, keepRevoked: Bool = false) -> Device? {
        switch d.state {
        case .revoked, .revokedByRelay: if !keepRevoked { return nil }
        case .verified, .unverified: break
        }
        return Device(
            id: d.id,
            name: d.name,
            platform: DevicePlatform(core: d.platform),
            lastActivity: d.lastSeenAtMs.map(date),
            isThisDevice: d.isThisDevice
        )
    }

    static func spaceState(_ s: SpaceStateKind) -> SpaceState {
        switch s {
        case .active: .active
        case .revoked: .revoked
        case .updateRequired: .updateRequired
        case .blocked: .blocked
        case .notFound: .notFound
        }
    }

    /// A hidden item carries no thumbnail, and all those of an item stay under `ThumbnailBudget.budgetBytes`.
    static func draft(_ d: OutgoingDraft) -> FfiDraft {
        let thumbnails = ThumbnailBudget.apply(d.attachments.map(\.thumbnail), secret: d.secret)
        return FfiDraft(
            id: d.id,
            title: d.title,
            text: d.text.map {
                TextContent(plain: $0.plain, html: $0.html, markdown: $0.markdown, sourceFormat: textFormat($0.sourceFormat))
            },
            url: d.url,
            attachments: zip(d.attachments, thumbnails).map { attachment, thumbnail in
                FfiDraftAttachment(
                    path: attachment.path, name: attachment.name, mime: attachment.mime,
                    thumbnail: thumbnail, thumbnailMime: thumbnail == nil ? nil : attachment.thumbnailMime
                )
            },
            secret: d.secret,
            expiresAtMs: d.expiresAt.map(ms),
            readPolicy: d.readPolicy == .afterFirstRead ? .afterFirstRead : .normal
        )
    }

    static func dispatch(_ k: DispatchKind) -> DispatchResult {
        switch k {
        case .sent: .sent
        case .retryLater: .retryLater
        case .notFound: .notFound
        case .failed: .failed
        }
    }

    static func pairing(_ p: PairingOutcome) -> PairingResult {
        switch p {
        case let .awaitingVerification(device, code, count, bytes, members, expiresIn, relayAuthority):
            let peer = PairingPeer(name: device.name, platform: DevicePlatform(core: device.platform))
            return .awaitingVerification(PairingPrompt(
                peer: peer, verificationCode: code, historyItemCount: Int(count),
                historyBytes: Int64(clamping: bytes), memberCount: members.map(Int.init), expiresInMs: expiresIn,
                relayAuthority: relayAuthority
            ))
        case let .failure(reason):
            switch reason {
            case .invalidCode: return .failure(.invalidCode)
            case .expired: return .failure(.expired)
            case .network: return .failure(.network)
            case .unsupported: return .failure(.unsupported)
            }
        }
    }

    static func departure(_ d: FfiDeparture) -> Departure {
        Departure(
            kind: d.kind == .leave ? .leave : .deleteSpace,
            wiping: d.wiping,
            since: d.sinceMs.map(date),
            last: d.last.map(pending)
        )
    }

    static func pending(_ r: FfiPendingReason) -> PendingReason {
        switch r {
        case .network: .network
        case .server: .server
        case .notFound: .notFound
        case .notSent: .notSent
        }
    }

    static func departureResult(_ o: FfiDepartureOutcome) -> DepartureResult {
        switch o {
        case .erased: return .erased
        case let .pending(reason): return .pending(pending(reason))
        case let .refused(reason):
            switch reason {
            case .lastDevice: return .refused(.lastDevice)
            case .updateRequired: return .refused(.updateRequired)
            case .relayTooOld: return .refused(.relayTooOld)
            case .unexpected: return .refused(.unexpected)
            }
        case .cancelled: return .cancelled
        case .nothingPending: return .nothingPending
        }
    }

    static func incoming(_ k: SimulatedIncoming) -> DemoIncomingKind {
        switch k {
        case .link: .link
        case .text: .text
        case .richText: .richText
        case .image: .image
        case .file: .file
        case .secret: .secret
        case .unreadable: .unreadable
        }
    }

    static func error(_ e: TomiError) -> TomiCoreError {
        switch e {
        case let .Storage(reason): TomiCoreError(.storage, reason: reason)
        case let .Network(reason): TomiCoreError(.network, reason: reason)
        case let .Relay(status, code): TomiCoreError(.relay(status: Int(status), code: code))
        case let .Blocked(reason): TomiCoreError(.blocked, reason: reason)
        case let .InvalidInput(reason): TomiCoreError(.invalidInput, reason: reason)
        case let .Crypto(code): TomiCoreError(.crypto(code: code))
        case let .Internal(reason): TomiCoreError(.internal, reason: reason)
        }
    }
}
