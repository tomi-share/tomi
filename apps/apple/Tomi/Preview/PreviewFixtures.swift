#if DEBUG
import Foundation
import TomiKit
import UIKit

/// Items written in Swift so that every card variant shows in `#Preview` and in the view-model tests, without the
/// Rust core. The app itself always runs on the real core (demo mode in Debug, 23 items); these never ship.
@MainActor
enum PreviewFixtures {
    static let phone = DeviceRef(id: "dev-iphone", name: "iPhone")
    static let office = DeviceRef(id: "dev-bureau", name: "PC-Bureau")
    static let home = DeviceRef(id: "dev-maison", name: "PC-Maison")
    static let now = Date()

    static func ago(_ minutes: Double) -> Date { now.addingTimeInterval(-minutes * 60) }

    static let link = TomiItem.readable(
        id: "link", sourceDevice: office, createdAt: ago(2), title: "Rust 2024 edition",
        url: "https://blog.rust-lang.org/2025/02/20/Rust-1.85.0.html"
    )
    static let plain = TomiItem.readable(
        id: "plain", sourceDevice: home, createdAt: ago(8),
        text: RichText(plain: "Penser à racheter du café, du lait d’avoine et des piles AA pour la télécommande.")
    )
    static let markdown = TomiItem.readable(
        id: "markdown", sourceDevice: office, createdAt: ago(15), title: "Notes de réunion",
        text: RichText(
            plain: "Notes de réunion",
            markdown: """
            ## Points
            - **Livraison** de la v1 vendredi
            - Relire la *spec* d’archive
            - `cargo test --features demo`

            > Ne pas oublier les captures.

            Voir [le ticket](https://example.com/t/42).
            """,
            sourceFormat: .markdown
        )
    )
    static let html = TomiItem.readable(
        id: "html", sourceDevice: home, createdAt: ago(40),
        text: RichText(
            plain: "Tableau des horaires",
            html: "<h3>Horaires</h3><table><tr><th>Jour</th><th>Heure</th></tr><tr><td>Lundi</td><td>9 h</td></tr><tr><td>Mardi</td><td>10 h</td></tr></table>",
            sourceFormat: .html
        )
    )
    static let image = TomiItem.readable(
        id: "image", sourceDevice: office, createdAt: ago(60),
        attachments: [Attachment(name: "capture-ecran.png", mime: "image/png", size: 482_113, thumbnail: thumbnail(), thumbnailMime: "image/png")]
    )
    static let files = TomiItem.readable(
        id: "files", sourceDevice: home, createdAt: ago(90), title: "Dossier projet",
        attachments: [
            Attachment(name: "devis-2026.pdf", mime: "application/pdf", size: 1_204_331),
            Attachment(name: "main.rs", mime: "text/x-rust", size: 4_120),
            Attachment(name: "budget.xlsx", mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", size: 88_004),
            Attachment(name: "notes.txt", mime: "text/plain", size: 912),
        ]
    )
    static let secret = TomiItem.readable(
        id: "secret", sourceDevice: office, createdAt: ago(5), title: "Wi-Fi maison",
        text: RichText(plain: "correct-horse-battery-staple"), secret: true
    )
    static let secretAfterRead = TomiItem.readable(
        id: "secret-once", sourceDevice: home, createdAt: ago(12),
        text: RichText(plain: "482913"), secret: true, readPolicy: .afterFirstRead
    )
    static let unreadable = TomiItem(id: "unreadable", content: .unreadable(.updateRequired), sourceDevice: home, createdAt: ago(130))
    static let outgoingQueued = TomiItem.readable(
        id: "out-queued", sourceDevice: phone, createdAt: ago(1),
        text: RichText(plain: "Envoyé depuis l’iPhone, en attente de réseau."),
        deliveries: [DeviceDelivery(deviceId: office.id, deviceName: office.name, state: .queued)]
    )
    static let outgoingDelivered = TomiItem.readable(
        id: "out-delivered", sourceDevice: phone, createdAt: ago(25), url: "https://www.apple.com/fr/",
        read: true,
        deliveries: [
            DeviceDelivery(deviceId: office.id, deviceName: office.name, state: .delivered),
            DeviceDelivery(deviceId: home.id, deviceName: home.name, state: .sent),
        ]
    )
    static let pinned = TomiItem.readable(
        id: "pinned", sourceDevice: office, createdAt: ago(600), title: "Adresse de livraison",
        text: RichText(plain: "12 rue des Lilas, 69003 Lyon"), read: true, pinned: true, pinnedAt: ago(500)
    )
    static let expiring = TomiItem.readable(
        id: "expiring", sourceDevice: home, createdAt: ago(30), text: RichText(plain: "Code de la porte : B1234"),
        expiresAt: now.addingTimeInterval(3 * 3600)
    )
    static let archived = TomiItem.readable(
        id: "archived", sourceDevice: office, createdAt: ago(3000), title: "Ancienne recette",
        text: RichText(plain: "Tarte aux pommes : 3 pommes, 1 pâte brisée."), read: true, archived: true, archivedAt: ago(100)
    )

    static let all: [TomiItem] = [
        outgoingQueued, link, secret, plain, secretAfterRead, markdown, outgoingDelivered, expiring, html, image, files,
        unreadable, pinned, archived,
    ]

    /// A feed model on an in-memory core; `items` defaults to every variant.
    static func model(items: [TomiItem] = all, online: Bool = true) -> FeedViewModel {
        FeedViewModel(
            core: PreviewCore(items: items, localDevice: phone),
            prefs: UserPrefs(defaults: UserDefaults(suiteName: "tomi.preview") ?? .standard),
            gate: BiometricGate { _ in .success },
            network: NetworkMonitor(),
            feedback: FeedbackCenter()
        )
    }

    /// A small gradient so the thumbnail of the image card is visible.
    static func thumbnail() -> Data? {
        UIGraphicsImageRenderer(size: CGSize(width: 320, height: 200)).pngData { context in
            let colors = [UIColor.systemTeal.cgColor, UIColor.systemIndigo.cgColor] as CFArray
            if let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1]) {
                context.cgContext.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: 320, y: 200), options: [])
            }
        }
    }
}

/// An in-memory core for previews and tests: pin, archive, read and delete change `state` the way the real core does
/// (pin and archive exclude each other); every call is recorded. Sending, pairing and devices answer what the test
/// sets (`cancelResult`, `dispatchResults`, `pairingResult`...).
@MainActor
final class PreviewCore: TomiCore {
    let state = CoreState()
    let isDemo = true
    private(set) var calls: [String] = []
    private(set) var enqueued: [OutgoingDraft] = []
    /// What `cancelIfQueued` answers: true while the item has not left.
    var cancelResult = true
    /// What the next `dispatch` calls answer, in order; `.sent` once empty.
    var dispatchResults: [DispatchResult] = []
    var enqueueError: Error?
    var pairingResult = PairingResult.failure(.invalidCode)
    var confirmError: Error?
    /// When set, `confirmPairing` waits for it (the other device has not confirmed yet).
    var confirmGate: Task<Void, Never>?
    var createError: Error?
    /// The relay this core says the device has (nil: none).
    var relay: String?

    init(items: [TomiItem], localDevice: DeviceRef) {
        state.isPaired = true
        state.localDevice = Device(id: localDevice.id, name: localDevice.name, platform: .ios, lastActivity: nil, isThisDevice: true)
        state.devices = [state.localDevice!] + [PreviewFixtures.office, PreviewFixtures.home].map {
            Device(id: $0.id, name: $0.name, platform: .windows, lastActivity: Date(), isThisDevice: false)
        }
        state.items = items.sorted { $0.createdAt > $1.createdAt }
        state.loaded = true
    }

    private func update(_ id: String, _ change: (inout TomiItem) -> Void) {
        guard let index = state.items.firstIndex(where: { $0.id == id }) else { return }
        change(&state.items[index])
    }

    func getItem(id: String) async throws -> TomiItem? { state.items.first { $0.id == id } }
    func sync(pushToken: String?) async throws { calls.append("sync") }
    func enqueue(_ draft: OutgoingDraft) async throws {
        calls.append("enqueue:\(draft.id)")
        if let enqueueError { throw enqueueError }
        enqueued.append(draft)
    }

    func dispatch(id: String) async throws -> DispatchResult {
        calls.append("dispatch:\(id)")
        return dispatchResults.isEmpty ? .sent : dispatchResults.removeFirst()
    }

    func cancelIfQueued(id: String) async throws -> Bool {
        calls.append("cancelIfQueued:\(id)")
        return cancelResult
    }

    func deleteLocal(id: String) async throws {
        calls.append("deleteLocal:\(id)")
        state.items.removeAll { $0.id == id }
    }

    func deleteEverywhere(id: String) async throws {
        calls.append("deleteEverywhere:\(id)")
        state.items.removeAll { $0.id == id }
    }

    func markRead(id: String) async throws {
        calls.append("markRead:\(id)")
        if state.items.first(where: { $0.id == id })?.readPolicy == .afterFirstRead {
            state.items.removeAll { $0.id == id }
        } else {
            update(id) { $0.read = true }
        }
    }

    func attachmentPath(itemId: String, slot: Int) async throws -> String? { nil }
    func renameDevice(id: String, name: String) async throws {
        calls.append("rename:\(id):\(name)")
        if let index = state.devices.firstIndex(where: { $0.id == id }) { state.devices[index].name = name }
    }

    func revokeDevice(id: String) async throws {
        calls.append("revoke:\(id)")
        state.devices.removeAll { $0.id == id }
    }

    func pin(id: String) async throws {
        calls.append("pin:\(id)")
        update(id) { $0.pinned = true; $0.pinnedAt = Date(); $0.archived = false; $0.archivedAt = nil }
    }

    func unpin(id: String) async throws {
        calls.append("unpin:\(id)")
        update(id) { $0.pinned = false; $0.pinnedAt = nil }
    }

    func archive(id: String) async throws {
        calls.append("archive:\(id)")
        update(id) { $0.archived = true; $0.archivedAt = Date(); $0.pinned = false; $0.pinnedAt = nil }
    }

    func unarchive(id: String) async throws {
        calls.append("unarchive:\(id)")
        update(id) { $0.archived = false; $0.archivedAt = nil }
    }

    func currentRelay() async -> String? { relay }

    /// Recorded without the relay when there is none, so that a test of the code alone reads `pairWithCode:<code>`; with
    /// one it reads `pairWithCode:<code>@<relay>`.
    func pairWithCode(code: String, relayUrl: String?) async throws -> PairingResult {
        calls.append("pairWithCode:\(code)" + (relayUrl.map { "@\($0)" } ?? ""))
        return pairingResult
    }

    func confirmPairing(importHistory: Bool) async throws {
        calls.append("confirmPairing:\(importHistory)")
        if let confirmGate { await confirmGate.value }
        if let confirmError { throw confirmError }
        state.isPaired = true
    }

    func cancelPairing() async { calls.append("cancelPairing") }

    func createSpace(installToken: String, deviceName: String, relayUrl: String?) async throws {
        calls.append("createSpace:\(deviceName)" + (relayUrl.map { "@\($0)" } ?? ""))
        if let createError { throw createError }
        state.isPaired = true
    }

    func resumePairing() async -> ResumeOutcome { .nothingPending }
    /// What `enterDemo` does: nil joins the demo, an error is thrown.
    var enterDemoError: Error?
    var canEnterDemo = false
    func enterDemo() async throws {
        calls.append("enterDemo")
        if let enterDemoError { throw enterDemoError }
        state.isPaired = true
        state.isDemo = true
    }

    // Departure: the preview core leaves at once, with the proof.
    var departureResults: [DepartureResult] = []
    func leaveSpace() async -> DepartureResult { departed("leave") }
    func deleteSpace() async -> DepartureResult { departed("delete") }
    func retryDeparture() async -> DepartureResult { departed("retry") }
    func continueDeparture() async -> DepartureResult {
        calls.append("continue")
        return state.departure == nil ? .nothingPending : departed("continue")
    }
    func cancelDeparture() async -> Bool {
        calls.append("cancelDeparture")
        defer { state.departure = nil }
        return state.departure != nil
    }
    func canEraseWithoutProof() async -> Bool { false }
    func eraseWithoutProof() async -> EraseResult { .notAllowed }
    func simulateDeparture(_ scenario: DemoDepartureScenario) async { calls.append("simulateDeparture") }

    private func departed(_ name: String) -> DepartureResult {
        calls.append(name)
        let result = departureResults.isEmpty ? DepartureResult.erased : departureResults.removeFirst()
        if result == .erased {
            state.departure = nil
            state.isPaired = false
        }
        return result
    }
    func startInvitation() async throws -> any InviteSession {
        calls.append("startInvitation")
        throw PairingFailure.unsupported
    }
    func isValidDeviceName(_ name: String) -> Bool {
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        return !trimmed.isEmpty && trimmed.unicodeScalars.count <= 64
    }
    func simulateIncoming(_ kind: SimulatedIncoming) async throws { calls.append("simulate:\(kind)") }
    func purgePlaintextCache() async throws {}
    func enterForeground() {}
    func enterBackground() {}
}
#endif
