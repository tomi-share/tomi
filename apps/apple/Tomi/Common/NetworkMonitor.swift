import Network
import Observation

/// Whether the device has a usable network. Only labels things ("Hors ligne · en attente",
/// "En attente de réseau"); it never blocks a send (the core retries by itself).
@MainActor
@Observable
final class NetworkMonitor {
    private(set) var isOnline = true
    private let monitor = NWPathMonitor()

    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let online = path.status == .satisfied
            Task { @MainActor in self?.isOnline = online }
        }
        monitor.start(queue: DispatchQueue(label: "eu.shaffe.tomi.network"))
    }

    deinit {
        monitor.cancel()
    }
}
