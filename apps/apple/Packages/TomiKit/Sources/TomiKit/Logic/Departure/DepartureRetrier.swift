import Foundation
import Observation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/core/DepartureRetrier.kt.

/// A refusal met by an automatic attempt: the user asked for this departure and must see that the relay refused it.
public struct DepartureRefusal: Sendable, Hashable {
    public let kind: DepartureKind
    public let reason: RefusalReason

    public init(kind: DepartureKind, reason: RefusalReason) {
        self.kind = kind
        self.reason = reason
    }
}

/// What the screens of the departure need from the automatic attempts.
@MainActor
public protocol DepartureFollowUp: AnyObject {
    /// A refusal met by an automatic attempt, until a screen showed it (`consumeRefusal`).
    var refusal: DepartureRefusal? { get }
    /// Called when `refusal` is set.
    var onRefusal: (() -> Void)? { get set }
    /// After a gesture that left the departure pending on the network or the server: (re)starts the series.
    func kick()
    func consumeRefusal()
}

/// The automatic attempts of a pending departure (spec/api-v1.md 7.9.4): after a gesture, `continueDeparture` at
/// `delays` (each plus 0 to 20 % at random) while it stays pending on the network or the server; then only when the
/// network or the app comes back (`comeBack`), at most once a minute. One series at a time. The core decides what an
/// attempt sends: within 15 minutes of the last gesture it sends the request again, later it only verifies. Nothing is
/// logged.
@MainActor
@Observable
public final class DepartureRetrier: DepartureFollowUp {
    public nonisolated static let defaultDelays: [Duration] = [.seconds(2), .seconds(5), .seconds(15), .seconds(30), .seconds(60), .seconds(120)]
    /// After the series: at most one attempt a minute when the network or the app comes back.
    public nonisolated static let comeBackMinGap: Duration = .seconds(60)
    private nonisolated static let maxJitter = 0.2

    public private(set) var refusal: DepartureRefusal?
    @ObservationIgnored public var onRefusal: (() -> Void)?

    @ObservationIgnored private let core: any TomiCore
    @ObservationIgnored private let delays: [Duration]
    @ObservationIgnored private let now: () -> ContinuousClock.Instant
    @ObservationIgnored private let random: () -> Double
    @ObservationIgnored private let sleep: (Duration) async throws -> Void
    @ObservationIgnored private var series: Task<Void, Never>?
    @ObservationIgnored private var lastComeBack: ContinuousClock.Instant?
    @ObservationIgnored private var comingBack = false

    public init(
        core: any TomiCore,
        delays: [Duration] = DepartureRetrier.defaultDelays,
        now: @escaping () -> ContinuousClock.Instant = { ContinuousClock.now },
        random: @escaping () -> Double = { Double.random(in: 0..<1) },
        sleep: @escaping (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    ) {
        self.core = core
        self.delays = delays
        self.now = now
        self.random = random
        self.sleep = sleep
    }

    /// At start-up, before the pairing is settled: one automatic attempt, then the series if it stays pending on the
    /// network or the server.
    @discardableResult
    public func settleAtStart() async -> DepartureResult {
        let result = await automaticAttempt()
        if result.isTransient { kick() }
        return result
    }

    public func kick() {
        series?.cancel()
        seriesId += 1
        let id = seriesId
        seriesRunning = true
        series = Task { [weak self] in
            guard let self else { return }
            defer { if id == self.seriesId { self.seriesRunning = false } }
            for base in delays {
                do { try await sleep(base + jitter(base)) } catch { return }
                guard !Task.isCancelled, id == seriesId, pending() else { return }
                guard await automaticAttempt().isTransient else { return }
            }
        }
    }

    /// A series of attempts is on.
    public private(set) var seriesRunning = false
    @ObservationIgnored private var seriesId = 0

    /// The device left the space (or the core is replaced): nothing more is tried, nothing is kept.
    public func stop() {
        series?.cancel()
        series = nil
        seriesId += 1
        seriesRunning = false
        refusal = nil
        onRefusal = nil
        lastComeBack = nil
    }

    public func consumeRefusal() {
        refusal = nil
    }

    /// The network or the app came back: one attempt, unless one runs or a series is on, and at most one a minute.
    public func comeBack() {
        guard !comingBack, !seriesRunning, !core.state.departureRunning, pending() else { return }
        let now = now()
        if let last = lastComeBack, last.duration(to: now) < Self.comeBackMinGap { return }
        lastComeBack = now
        comingBack = true
        Task {
            _ = await automaticAttempt()
            comingBack = false
        }
    }

    private func automaticAttempt() async -> DepartureResult {
        let kind = core.state.departure?.kind
        let result = await core.continueDeparture()
        if case let .refused(reason) = result, let kind {
            refusal = DepartureRefusal(kind: kind, reason: reason)
            onRefusal?()
        }
        return result
    }

    private func pending() -> Bool {
        guard let departure = core.state.departure else { return false }
        return !departure.wiping
    }

    private func jitter(_ base: Duration) -> Duration {
        base * (random() * Self.maxJitter)
    }
}
