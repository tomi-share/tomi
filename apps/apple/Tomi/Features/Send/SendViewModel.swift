import Foundation
import Observation
import TomiKit
import TomiRuntime

/// Why the "Masquer le contenu" switch is on, told under it.
enum SecretHint: Equatable {
    case none
    /// The copying app marked the content as concealed (password managers): the switch is locked on.
    case sourceSensitive
    /// It looks like a password or a code (`SecretDetector`).
    case heuristic
}

/// A file chosen with "Joindre", already copied into the app (`CorePaths.picked`): the system only lends a picked file
/// for the moment of the choice.
struct PickedFile: Identifiable, Hashable {
    let id = UUID()
    let file: SharedFile
}

/// State of the send screen (Android `SendViewModel`). The clipboard is read only by the "Coller" button, which hands
/// what it got to `onPasted`. Lives as long as the app: a draft left on the screen is still there when coming back.
@MainActor
@Observable
final class SendViewModel {
    static let maxAttachments = ShareLimits.maxFiles

    private(set) var title = ""
    private(set) var text = ""
    private(set) var secret = false
    private(set) var secretLocked = false
    private(set) var secretHint = SecretHint.none
    private(set) var expiry = ExpiryPolicy.defaultChoice
    private(set) var attachments: [PickedFile] = []
    private(set) var submission: Submission?
    /// Transient message (a text key), shown by the screen then consumed.
    private(set) var notice: String?
    private(set) var sending = false
    /// "Annuler" is running: the banner's buttons are off and its end waits.
    private(set) var undoing = false
    /// One-shot: an item was just sent, the screen hands over to the feed (where the item shows up, with the
    /// "Envoyé / Annuler" banner). Consumed by the screen with `consumeLeaveAfterSend`.
    private(set) var leaveAfterSend = false

    private let core: any TomiCore
    private let coordinator: SendCoordinator
    private let picked: PickedFiles
    private var pastedHTML: String?
    private var snapshot: Snapshot?
    /// Ends the undo window on the deadline, whatever the screen shows (the banner only draws it).
    private var timeout: Task<Void, Never>?

    private struct Snapshot {
        let title: String
        let text: String
        let html: String?
        let attachments: [PickedFile]
        let secret: Bool
        let locked: Bool
        let hint: SecretHint
        let expiry: ExpiryChoice
    }

    init(core: any TomiCore, coordinator: SendCoordinator, picked: PickedFiles) {
        self.core = core
        self.coordinator = coordinator
        self.picked = picked
    }

    /// Names of the other devices of the space ("Vers PC-Bureau, PC-Maison").
    var recipients: [String] {
        core.state.devices.filter { !$0.isThisDevice }.map(\.name)
    }

    var hasAttachment: Bool { !attachments.isEmpty }

    var canSend: Bool {
        (!title.isBlank || !text.isBlank || !attachments.isEmpty) && !sending
    }

    /// The form can be changed: not while the draft is being copied and handed to the core.
    var editable: Bool { !sending }

    var expiryOptions: [ExpiryChoice] {
        ExpiryPolicy.options(secret: secret, hasAttachment: hasAttachment)
    }

    /// `tomi-share://send` pre-fill: only fills the fields, never sends.
    func prefill(title: String?, url: String?, text: String?, droppedParameters: Bool) {
        dropSnapshot()
        submission = nil
        self.title = title ?? ""
        removeAllAttachments()
        self.text = [url, text].compactMap { $0 }.joined(separator: "\n")
        pastedHTML = nil
        resetSecret()
        notice = droppedParameters ? "notice_params_dropped" : nil
    }

    func onTitleChange(_ value: String) {
        guard editable else { return }
        title = String(value.prefix(SharePayloadParser.maxTitle))
    }

    func onTextChange(_ value: String) {
        guard editable else { return }
        text = String(value.prefix(SharePayloadParser.maxText))
        pastedHTML = nil
        if text.isEmpty { resetSecret() }
    }

    func onSecretChange(_ value: Bool) {
        if secretLocked && !value { return }
        secret = value
        expiry = ExpiryPolicy.onContentChanged(current: expiry, secret: value, hasAttachment: hasAttachment)
        if !value { secretHint = .none }
    }

    func onExpiryChange(_ choice: ExpiryChoice) {
        if ExpiryPolicy.isAllowed(choice, secret: secret, hasAttachment: hasAttachment) { expiry = choice }
    }

    /// What the user just pasted with "Coller". `concealed`: the copying app marked it as such (password managers):
    /// the content is then hidden, and the switch locked on.
    func onPasted(plain: String?, html: String?, concealed: Bool) {
        guard editable else { return }
        guard let clean = plain.map({ String($0.prefix(SharePayloadParser.maxText)) }), !clean.isEmpty else {
            notice = "notice_paste_empty"
            return
        }
        let wasEmpty = text.isBlank
        text = wasEmpty ? clean : String((text + "\n" + clean).prefix(SharePayloadParser.maxText))
        pastedHTML = wasEmpty ? html : nil
        if concealed {
            secret = true
            secretLocked = true
            secretHint = .sourceSensitive
            expiry = ExpiryPolicy.onContentChanged(current: expiry, secret: true, hasAttachment: hasAttachment)
        } else if SecretDetector.looksLikeSecret(clean) && !secret {
            secret = true
            secretHint = .heuristic
            expiry = ExpiryPolicy.onContentChanged(current: expiry, secret: true, hasAttachment: hasAttachment)
        }
    }

    func onAttach(_ files: [SharedFile], skipped: Int = 0) {
        guard editable else {
            files.forEach { picked.delete($0) }
            return
        }
        let room = Self.maxAttachments - attachments.count
        let added = files.prefix(max(room, 0)).map { PickedFile(file: $0) }
        files.dropFirst(max(room, 0)).forEach { picked.delete($0) }
        attachments += added
        // "Après lecture" would destroy the attached file as soon as it is opened: not offered with a file.
        expiry = ExpiryPolicy.onContentChanged(current: expiry, secret: secret, hasAttachment: hasAttachment)
        if skipped > 0 || files.count > added.count { notice = "ios_notice_files_skipped" }
    }

    func onRemoveAttachment(_ id: PickedFile.ID) {
        guard editable, let index = attachments.firstIndex(where: { $0.id == id }) else { return }
        picked.delete(attachments.remove(at: index).file)
    }

    func consumeNotice() {
        notice = nil
    }

    func consumeLeaveAfterSend() {
        leaveAfterSend = false
    }

    /// The screen shows: a send that ended while it was not on screen does not close it.
    func screenAppeared() {
        leaveAfterSend = false
    }

    func send() async {
        guard canSend else { return }
        sending = true
        defer { sending = false }
        let input = ShareInput(
            subject: title, text: text, htmlText: pastedHTML,
            files: attachments.map(\.file), markedSensitive: secretLocked
        )
        let parsed = SharePayloadParser.parse(input)
        guard !parsed.isEmpty else { return }
        let current = Snapshot(
            title: title, text: text, html: pastedHTML, attachments: attachments,
            secret: secret, locked: secretLocked, hint: secretHint, expiry: expiry
        )
        do {
            let sent = try await coordinator.submit(parsed, secret: secret, expiry: expiry)
            dropSnapshot()
            snapshot = current
            submission = sent
            startTimeout(sent)
            title = ""
            text = ""
            pastedHTML = nil
            attachments = []
            resetSecret()
            leaveAfterSend = true
        } catch {
            // Any failure of the copy or of the core: what the user typed stays.
            notice = "notice_send_failed"
        }
    }

    /// "Annuler" (and "Options"): cancels the send and gives the draft back, or says it is too late if the item already
    /// left (the core decides). Nothing is deleted then. Runs once: a second touch, or the end of the window meanwhile,
    /// does nothing.
    func undo() async {
        guard !undoing, let sent = submission else { return }
        undoing = true
        timeout?.cancel()
        let saved = snapshot
        snapshot = nil
        let outcome = try? await coordinator.cancel(sent)
        submission = nil
        undoing = false
        switch outcome {
        case .cancelled:
            if let saved { restore(saved) }
        case .tooLate:
            saved.map(deleteFiles)
            notice = "notice_too_late"
        case nil:
            saved.map(deleteFiles)
            notice = "notice_undo_failed"
        }
    }

    /// The undo window is over: the item is on its way, and the copy kept for "Annuler" (a secret, maybe) is dropped.
    func onBannerTimeout() {
        guard !undoing else { return }
        timeout?.cancel()
        submission = nil
        dropSnapshot()
    }

    private func startTimeout(_ sent: Submission) {
        timeout?.cancel()
        timeout = Task { [weak self] in
            let remaining = sent.deadline.remainingMs(MonotonicClock.nowMs())
            try? await Task.sleep(for: .milliseconds(remaining))
            guard !Task.isCancelled, let self, self.submission?.id == sent.id else { return }
            self.onBannerTimeout()
        }
    }

    /// The draft comes back in place of whatever was started meanwhile (its chosen files are dropped).
    private func restore(_ saved: Snapshot) {
        let kept = Set(saved.attachments.map(\.id))
        attachments.filter { !kept.contains($0.id) }.forEach { picked.delete($0.file) }
        title = saved.title
        text = saved.text
        pastedHTML = saved.html
        attachments = saved.attachments
        secret = saved.secret
        secretLocked = saved.locked
        secretHint = saved.hint
        expiry = saved.expiry
    }

    private func resetSecret() {
        secret = false
        secretLocked = false
        secretHint = .none
        expiry = ExpiryPolicy.defaultChoice
    }

    private func removeAllAttachments() {
        attachments.forEach { picked.delete($0.file) }
        attachments = []
    }

    private func dropSnapshot() {
        snapshot.map(deleteFiles)
        snapshot = nil
    }

    private func deleteFiles(_ saved: Snapshot) {
        saved.attachments.forEach { picked.delete($0.file) }
    }
}
