import Foundation
import TomiCore
import TomiKit

/// `InviteSession` on the core's `FfiInviteSession` (Android `RealTomiCore` invitation): the waiting calls block on a
/// queue of their own; `cancel` goes through another, so that it ends a wait instead of queueing behind it. A transient
/// error (network, relay busy) leaves the session alive: the waiting calls try again after 1, 2 and 4 s, so that a
/// phone that changes networks in the middle of a long poll does not cost the user a new code.
final class RustInviteSession: InviteSession, @unchecked Sendable {
    private let inner: FfiInviteSession
    private let waits = CoreQueue(label: "eu.shaffe.tomi.core.invite", qos: .userInitiated)
    private let control = CoreQueue(label: "eu.shaffe.tomi.core.invite.cancel", qos: .userInitiated)

    init(_ inner: FfiInviteSession) {
        self.inner = inner
    }

    var code: String? { inner.code() }
    var link: String? { inner.link() }
    func expiresInMs() -> Int64 { max(inner.expiresInMs(), 0) }

    func awaitPeer() async throws -> InvitePrompt {
        let prompt = try await blocking(retrying: true) { try $0.waitForPeer() }
        return InvitePrompt(
            peer: PairingPeer(name: prompt.peer.name, platform: DevicePlatform(core: prompt.peer.platform)),
            verificationCode: prompt.sas.filter(\.isNumber)
        )
    }

    func confirm() async throws {
        try await blocking(retrying: false) { try $0.confirm() }
    }

    func awaitDone() async throws -> InvitedDevice {
        let outcome = try await blocking(retrying: true) { try $0.waitDone() }
        return InvitedDevice(
            deviceId: outcome.deviceId, name: outcome.name, platform: DevicePlatform(core: outcome.platform),
            historyAnnounced: outcome.history != nil
        )
    }

    func cancel() async {
        let inner = self.inner
        _ = try? await control.run { inner.cancel() }
    }

    private func blocking<T: Sendable>(
        retrying: Bool, _ body: @escaping @Sendable (FfiInviteSession) throws -> T
    ) async throws -> T {
        let inner = self.inner
        do {
            return try await waits.run {
                var attempt = 0
                while true {
                    do {
                        return try body(inner)
                    } catch let error as TomiError {
                        let mapped = CoreMapping.error(error)
                        guard retrying, PairingErrors.isTransient(mapped), attempt < PairingErrors.retryDelaysMs.count else {
                            throw mapped
                        }
                        Thread.sleep(forTimeInterval: Double(PairingErrors.retryDelaysMs[attempt]) / 1000)
                        attempt += 1
                    }
                }
            }
        } catch let error as TomiCoreError {
            throw PairingErrors.failure(of: error)
        }
    }
}
