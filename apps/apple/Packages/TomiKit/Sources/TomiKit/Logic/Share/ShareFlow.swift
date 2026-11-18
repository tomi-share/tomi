import Foundation
import Observation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/share/ShareViewModel.kt.

/// A share handed over for sending, with what "Annuler" and "Options" need.
public struct ShareSent: Hashable, Sendable {
    public let id: String
    public let parsed: ParsedShare
    public let secret: Bool
    public let expiry: ExpiryChoice
    public let skippedFiles: Int
    /// End of the undo window: the banner closes then.
    public let deadline: UndoDeadline

    public init(id: String, parsed: ParsedShare, secret: Bool, expiry: ExpiryChoice, skippedFiles: Int, deadline: UndoDeadline) {
        self.id = id
        self.parsed = parsed
        self.secret = secret
        self.expiry = expiry
        self.skippedFiles = skippedFiles
        self.deadline = deadline
    }
}

/// Hands a share over for sending and takes it back.
@MainActor
public protocol ShareSending: AnyObject {
    func submit(_ parsed: ParsedShare, secret: Bool, expiry: ExpiryChoice) async throws -> ShareSent
    /// "Annuler": false when the share already left (too late, nothing was deleted).
    func cancel(_ sent: ShareSent) async throws -> Bool
}

/// The settings of a share before it is sent: the confirmation of what looks like a secret, or "Options".
public struct ShareDraft: Hashable, Sendable {
    public var parsed: ParsedShare
    public var secret: Bool
    public var expiry: ExpiryChoice

    public var hasAttachment: Bool { !parsed.files.isEmpty }
    public var expiryOptions: [ExpiryChoice] { ExpiryPolicy.options(secret: secret, hasAttachment: hasAttachment) }
}

public enum ShareState: Hashable, Sendable {
    case loading
    /// This device has no space yet: nothing can be sent.
    case notPaired
    /// The app could not open its data ("Données illisibles"): the user is sent to the app to repair it.
    case unreadable
    /// The share carried nothing usable.
    case nothingToShare
    /// "Annuler" came too late: the item already left, nothing was deleted.
    case tooLate
    case failed
    /// What looks like a secret never leaves on a single tap: confirmed first. `secretLocked`: the source marked it
    /// sensitive, the switch cannot be turned off.
    case confirm(ShareDraft, secretLocked: Bool)
    /// Handed over; the item leaves after the undo window.
    case sent(ShareSent)
    /// "Options" of the banner: the previous send was cancelled.
    case options(ShareDraft)

    /// A secret is on screen (its preview is masked, but the sheet still covers it in the app switcher).
    public var showsSecret: Bool {
        switch self {
        case let .confirm(draft, _), let .options(draft): draft.secret
        default: false
        }
    }
}

/// The share sheet of Tomi (Android `ShareViewModel`): sent at once with the default settings and a banner offering
/// "Annuler" and "Options", or confirmed first when it looks like a secret.
@MainActor
@Observable
public final class ShareFlow {
    public private(set) var state = ShareState.loading
    /// A cancel is running: the banner buttons wait.
    public private(set) var busy = false
    @ObservationIgnored private let sender: ShareSending
    @ObservationIgnored private let isPaired: Bool
    @ObservationIgnored private let isUnreadable: Bool
    @ObservationIgnored private var started = false

    public init(sender: ShareSending, isPaired: Bool, isUnreadable: Bool = false) {
        self.sender = sender
        self.isPaired = isPaired
        self.isUnreadable = isUnreadable
    }

    /// Reads the share (`load` copies its files) and sends it or asks first. Runs once.
    public func start(load: @escaping () async throws -> ShareInput) async {
        guard !started else { return }
        started = true
        // Android `ShareActivity`: a core that cannot be opened is said before anything else.
        guard !isUnreadable else {
            state = .unreadable
            return
        }
        guard isPaired else {
            state = .notPaired
            return
        }
        let input: ShareInput
        do {
            input = try await load()
        } catch {
            state = .failed
            return
        }
        let parsed = SharePayloadParser.parse(input)
        if parsed.isEmpty {
            state = .nothingToShare
        } else if parsed.looksSecret {
            let draft = ShareDraft(parsed: parsed, secret: true, expiry: ExpiryPolicy.defaultChoice)
            state = .confirm(draft, secretLocked: input.markedSensitive)
        } else {
            await submit(ShareDraft(parsed: parsed, secret: false, expiry: ExpiryPolicy.defaultChoice))
        }
    }

    public func onSecretChange(_ value: Bool) {
        update { draft, locked in
            guard !locked || value else { return }
            draft.secret = value
            draft.expiry = ExpiryPolicy.onContentChanged(current: draft.expiry, secret: value, hasAttachment: draft.hasAttachment)
        }
    }

    public func onExpiryChange(_ choice: ExpiryChoice) {
        update { draft, _ in
            if ExpiryPolicy.isAllowed(choice, secret: draft.secret, hasAttachment: draft.hasAttachment) { draft.expiry = choice }
        }
    }

    /// "Envoyer" of the confirmation or of "Options".
    public func confirm() async {
        switch state {
        case let .confirm(draft, _), let .options(draft): await submit(draft)
        default: break
        }
    }

    /// "Annuler" of the banner. True when the sheet can close (cancelled); false when it shows why not.
    public func undo() async -> Bool {
        guard case let .sent(sent) = state else { return true }
        return await cancelSucceeded(sent)
    }

    /// "Options" of the banner: cancels the send and opens its settings.
    public func openOptions() async {
        guard case let .sent(sent) = state, await cancelSucceeded(sent) else { return }
        state = .options(ShareDraft(parsed: sent.parsed, secret: sent.secret, expiry: sent.expiry))
    }

    private func cancelSucceeded(_ sent: ShareSent) async -> Bool {
        guard !busy else { return false }
        busy = true
        defer { busy = false }
        do {
            guard try await sender.cancel(sent) else {
                state = .tooLate
                return false
            }
            return true
        } catch {
            state = .failed
            return false
        }
    }

    private func submit(_ draft: ShareDraft) async {
        state = .loading
        do {
            state = .sent(try await sender.submit(draft.parsed, secret: draft.secret, expiry: draft.expiry))
        } catch {
            // Any failure (copy, storage) ends in the "failed" card, never a crash.
            state = .failed
        }
    }

    private func update(_ change: (inout ShareDraft, Bool) -> Void) {
        switch state {
        case .confirm(var draft, let locked):
            change(&draft, locked)
            state = .confirm(draft, secretLocked: locked)
        case .options(var draft):
            change(&draft, false)
            state = .options(draft)
        default:
            break
        }
    }
}
