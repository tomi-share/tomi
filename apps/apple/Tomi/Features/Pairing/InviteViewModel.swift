import Foundation
import Observation
import TomiKit

/// "Ajouter un appareil" (Android `InviteViewModel`): runs `InviteFlow` for the screen and shows its states. Leaving the
/// screen, by any way, ends the invitation (the channel is closed at the relay).
@MainActor
@Observable
final class InviteViewModel {
    private(set) var state = InviteState.starting
    @ObservationIgnored private let core: any TomiCore
    /// The current attempt; "Nouveau code" starts a new one (a flow runs once).
    @ObservationIgnored private var flow: InviteFlow?
    @ObservationIgnored private var job: Task<Void, Never>?
    @ObservationIgnored private var started = false

    init(core: any TomiCore) {
        self.core = core
    }

    /// When the screen appears; once (coming back to it does not restart anything).
    func start() {
        guard !started else { return }
        started = true
        launch()
    }

    /// "Nouveau code" after a failure.
    func restart() {
        abort()
        launch()
    }

    /// "Ajouter": the codes are the same. Both people must confirm before anything is given.
    func add() { flow?.add() }

    /// "Ils sont différents", or back from the comparison: a new code is made.
    func refuse() { flow?.refuse() }

    /// "Annuler", or leaving the screen: stops everything and closes the channel at the relay.
    func abort() {
        job?.cancel()
        job = nil
        flow?.abort()
    }

    private func launch() {
        let flow = InviteFlow(core: core) { [weak self] in self?.state = $0 }
        self.flow = flow
        state = .starting
        job = Task { await flow.run() }
    }
}
