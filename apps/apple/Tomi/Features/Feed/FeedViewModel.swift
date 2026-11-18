import Foundation
import Observation
import TomiKit

/// State and actions of the feed, the detail and (later) the viewers. Port of `FeedViewModel.kt`.
@MainActor
@Observable
final class FeedViewModel {
    static let autoHideSeconds: Double = 30
    static let maxQuery = 200

    let core: any TomiCore
    private let prefs: UserPrefs
    let gate: BiometricGate
    private let network: NetworkMonitor
    let feedback: FeedbackCenter

    var filter = FeedFilter.all
    private(set) var query = ""
    private(set) var preferredRenderMode: RenderMode?
    /// Hidden items currently shown in clear, with the item as it was when revealed: an "after first read" item leaves
    /// the core as soon as it is read, and stays on screen until it is hidden.
    private(set) var revealed: [String: TomiItem] = [:]
    private var hideTasks: [String: Task<Void, Never>] = [:]
    /// The one card whose archive pill is open (opening another, or scrolling, closes it).
    var openSwipeItemId: String?
    /// File to preview with Quick Look (attachments, until the viewers of PR-9).
    /// The file viewer (documents, audio, video, files Tomi cannot draw), over everything.
    var fileRequest: ViewerRequest?
    /// The image viewer over the stack, and the code screen to push (a tap on an image or a code file).
    var viewerRequest: ViewerRequest?
    var codeRequest: ViewerRequest?
    /// The relay takes "remove this file everywhere" (`TomiCore.supportsAttachmentRemoval`); the screens offer that scope
    /// only then. False until `refreshAttachmentRemovalSupport` heard the core.
    var attachmentRemovalEverywhere = false

    init(core: any TomiCore, prefs: UserPrefs, gate: BiometricGate, network: NetworkMonitor, feedback: FeedbackCenter) {
        self.core = core
        self.prefs = prefs
        self.gate = gate
        self.network = network
        self.feedback = feedback
        preferredRenderMode = prefs.preferredRenderMode
    }

    // MARK: State

    var localDeviceId: String { core.state.localDevice?.id ?? "" }
    var isOnline: Bool { network.isOnline }
    var loaded: Bool { core.state.loaded }
    var revealedIds: Set<String> { Set(revealed.keys) }

    /// Items of the core plus the revealed ones that already left it, newest first.
    var allItems: [TomiItem] {
        let items = core.state.items
        let missing = revealed.values.filter { snapshot in !items.contains { $0.id == snapshot.id } }
        return (items + missing).sorted { $0.createdAt > $1.createdAt }
    }

    var visibleItems: [TomiItem] { FeedFiltering.apply(allItems, filter, query) }
    var sections: FeedSections { FeedSections.split(visibleItems) }

    /// "Hors ligne" shows only when it matters: offline with something still queued here.
    var showsOfflineChip: Bool {
        !isOnline && allItems.contains { item in
            item.isOutgoing(localDeviceId: localDeviceId) && item.deliveries.contains { $0.state == .queued }
        }
    }

    func item(id: String) -> TomiItem? { allItems.first { $0.id == id } }

    func setQuery(_ value: String) {
        query = String(value.prefix(Self.maxQuery))
    }

    func setRenderMode(_ mode: RenderMode) {
        prefs.preferredRenderMode = mode
        preferredRenderMode = mode
    }

    func isRevealed(_ item: TomiItem) -> Bool { revealed[item.id] != nil }

    // MARK: Reading

    /// The user opened or tapped a card. A hidden item is only marked read once revealed or copied.
    func onCardOpened(_ item: TomiItem) {
        guard item.shouldMarkReadWhenOpened(localDeviceId: localDeviceId) else { return }
        Task { try? await core.markRead(id: item.id) }
    }

    /// Cards that stayed on screen (see `SeenTracker`) are read. Same rule as a tap: never a hidden item (revealing it
    /// is the read), an unreadable one or one sent from this device. A failure is silent, as for a tap.
    func onItemsSeen(_ items: [TomiItem]) {
        let localId = localDeviceId
        let unread = items.filter { $0.shouldMarkReadWhenOpened(localDeviceId: localId) }
        if unread.isEmpty { return }
        Task {
            for item in unread { try? await core.markRead(id: item.id) }
        }
    }

    func reveal(_ item: TomiItem) {
        Task {
            switch await gate.authenticate(reason: L10n.text("biometric_title_secret")) {
            case .success:
                revealed[item.id] = item
                scheduleHide(item.id)
                if !item.read || item.readPolicy == .afterFirstRead { try? await core.markRead(id: item.id) }
            case .unavailable:
                feedback.show(L10n.text("toast_biometric_unavailable"))
            case .cancelled, .failed:
                break
            }
        }
    }

    func hide(_ itemId: String) {
        hideTasks.removeValue(forKey: itemId)?.cancel()
        revealed.removeValue(forKey: itemId)
    }

    /// Hides everything: the app leaves the screen.
    func hideAll() {
        hideTasks.values.forEach { $0.cancel() }
        hideTasks.removeAll()
        revealed.removeAll()
    }

    private func scheduleHide(_ itemId: String) {
        hideTasks[itemId]?.cancel()
        hideTasks[itemId] = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.autoHideSeconds))
            guard !Task.isCancelled else { return }
            self?.hide(itemId)
        }
    }

    // MARK: Copy, share

    /// Copies exactly what the card shows. A hidden item needs authentication first (unless revealed).
    func copy(_ item: TomiItem, mode: RenderMode? = nil) {
        Task {
            if item.secret && !isRevealed(item) {
                switch await gate.authenticate(reason: L10n.text("biometric_title_copy_secret")) {
                case .success: break
                case .unavailable:
                    feedback.show(L10n.text("toast_biometric_unavailable"))
                    return
                case .cancelled, .failed: return
                }
            }
            let plain: String
            var html: String?
            if let text = item.text {
                let content = RenderModes.copyContent(text, mode: mode ?? RenderModes.resolve(text, preferred: preferredRenderMode))
                plain = content.plain
                html = content.html
            } else if let url = item.url {
                plain = url
            } else {
                return
            }
            SensitiveClipboard.copy(plain: plain, html: html, sensitive: item.secret)
            if !item.read || item.readPolicy == .afterFirstRead { try? await core.markRead(id: item.id) }
            feedback.show(L10n.text(item.secret ? "snackbar_copied_secret" : "snackbar_copied"))
        }
    }

    /// Copies only the link of a composite item.
    func copyLink(_ item: TomiItem) {
        // Never a hidden item's link in clear on a single tap (Android L2): "Copier" goes through the gate.
        guard !item.secret, let url = item.url else { return }
        SensitiveClipboard.copy(plain: url, sensitive: false)
        feedback.show(L10n.text("snackbar_copied"))
    }

    func share(_ item: TomiItem) {
        Task {
            let outcome = await ItemSharing.prepare(item) {
                await self.gate.authenticate(reason: L10n.text("biometric_title_share"))
            }
            switch outcome {
            case let .ready(payload):
                let opened = SystemShare.present(text: payload.text, subject: payload.subject, sensitive: item.secret)
                if opened && (!item.read || item.readPolicy == .afterFirstRead) { try? await core.markRead(id: item.id) }
            case .nothingToShare:
                feedback.show(L10n.text("snackbar_nothing_to_share"))
            case .biometricUnavailable:
                feedback.show(L10n.text("toast_biometric_unavailable"))
            case .notAuthenticated:
                break
            }
        }
    }

    // MARK: Pin, archive, delete

    func togglePin(_ item: TomiItem) {
        Task {
            do {
                if item.pinned { try await core.unpin(id: item.id) } else { try await core.pin(id: item.id) }
            } catch {
                feedback.show(L10n.text("snackbar_pin_failed"))
            }
        }
    }

    /// Archives or brings back (swipe and menu), then offers "Annuler". Never marks the item read.
    func applyArchiveAction(_ item: TomiItem, _ action: ArchiveAction? = nil) {
        let action = action ?? ArchiveSwipe.menuAction(item)
        if action == .archive { hide(item.id) }
        if openSwipeItemId == item.id { openSwipeItemId = nil }
        Task {
            do {
                try await ArchiveSwipe.perform(core, action, itemId: item.id)
                let wasPinned = item.pinned
                feedback.show(
                    L10n.text(action == .archive ? "snackbar_archived" : "snackbar_unarchived"),
                    actionTitle: L10n.text("action_undo")
                ) { self.undoArchive(action, itemId: item.id, wasPinned: wasPinned) }
            } catch {
                feedback.show(L10n.text("snackbar_archive_failed"))
            }
        }
    }

    private func undoArchive(_ action: ArchiveAction, itemId: String, wasPinned: Bool) {
        Task {
            do {
                try await ArchiveSwipe.undo(core, action, itemId: itemId, wasPinned: wasPinned)
            } catch {
                feedback.show(L10n.text("snackbar_archive_failed"))
            }
        }
    }

    func deleteLocal(_ item: TomiItem) {
        hide(item.id)
        Task {
            // "Supprimé" only once it is (Android `FeedEvent.ActionFailed`).
            do {
                try await core.deleteLocal(id: item.id)
                feedback.show(L10n.text("snackbar_deleted_local"))
            } catch {
                feedback.show(L10n.text("snackbar_action_failed"))
            }
        }
    }

    func deleteEverywhere(_ item: TomiItem) {
        hide(item.id)
        Task {
            do {
                try await core.deleteEverywhere(id: item.id)
                feedback.show(L10n.text("snackbar_deleted_everywhere"))
            } catch {
                feedback.show(L10n.text("snackbar_action_failed"))
            }
        }
    }

    // MARK: Attachments

    /// Opens attachment `index` of `item` in the file viewer (files other than images and code). A hidden item's file
    /// is never drawn there (Quick Look and the player draw out of reach of the capture shield): the viewer offers to
    /// share it behind the gate.
    func openAttachment(_ item: TomiItem, index: Int) {
        guard item.attachments.indices.contains(index) else { return }
        if !item.secret { onCardOpened(item) }
        viewerRequest = nil
        fileRequest = ViewerRequest(itemId: item.id, attachmentIndex: index)
    }

    // MARK: Demo

    func simulateIncoming(_ kind: SimulatedIncoming) {
        Task { try? await core.simulateIncoming(kind) }
    }
}
