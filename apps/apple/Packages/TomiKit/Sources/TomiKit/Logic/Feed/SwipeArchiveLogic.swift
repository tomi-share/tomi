import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/SwipeArchiveLogic.kt (pure parts only: the Compose
// anchoredDraggable state and the settle animation are replaced by the gesture of the app; units are points).

/// The three resting places of a card that is swiped to reveal its action (two steps, as in iOS Mail / Gmail):
///  - `resting`: offset 0, closed;
///  - `open`: offset `-actionsWidthDp` (88 pt), the pill is fully revealed, settled and tappable, nothing is archived yet;
///  - `out`: offset `-width`, the card is fully out to the LEFT = the action is committed (archive / un-archive).
/// There is NO anchor on the right, so a drag to the right can not move the card past `resting`.
public enum SwipeValue: Sendable, Hashable, CaseIterable {
    case resting, open, out
}

/// Where the finger (or the card) is, by distance dragged: `closed` (the pill has not fully entered), `open` (the pill
/// is fully there), `armed` (past 58 % of the width: releasing now runs the action). It drives the visual, the haptic
/// tick and the decision at the release, so that what the user feels is what happens.
public enum SwipeZone: Sendable, Hashable, CaseIterable {
    case closed, open, armed
}

/// The positions (offsets, negative to the left) of the three resting places (Compose `DraggableAnchors`).
public struct SwipeAnchors: Sendable, Hashable {
    public let resting: Float
    public let open: Float
    public let out: Float

    public init(resting: Float, open: Float, out: Float) {
        self.resting = resting
        self.open = open
        self.out = out
    }

    public var count: Int { 3 }

    public func positionOf(_ value: SwipeValue) -> Float {
        switch value {
        case .resting: resting
        case .open: open
        case .out: out
        }
    }

    /// The smallest position (fully out to the left).
    public var minPosition: Float { min(resting, min(open, out)) }
    /// The largest position: rest. Nothing on the right.
    public var maxPosition: Float { max(resting, max(open, out)) }
}

/// Pure rules of the swipe-to-archive, free of any gesture code so that they can be unit tested. The behaviour and the
/// timings are the ones of the reference app (T3 Code, `thread-swipe-actions`):
///
///  - the decision is taken WHEN THE FINGER IS LIFTED, by how far the card was dragged, never by the velocity:
///    below the opening stage (`actionsWidthDp`, the pill is not fully in) the card closes; from the opening stage to
///    `thresholdFraction` of the width it stays `.open` (the pill is tappable); past the threshold it is "armed" and the
///    release runs the action. Crossing the threshold while dragging only gives a haptic tick (the only one), it never
///    runs anything;
///  - the threshold is `thresholdFraction` of the card width (T3: `0.58`);
///  - from `.open` a tap on the pill runs the same action as the full swipe, a tap on the card or a drag to the right
///    closes it.
public enum SwipeArchiveLogic {
    /// Share of the card width to drag before the release runs the action (T3: 0.58).
    public static let thresholdFraction: Float = 0.58

    /// Share of the card width from which the icon is stuck to the left edge of the stretching pill (before, it travels
    /// there from 58 %).
    public static let reachFraction: Float = 0.75

    /// Once armed, the card is only disarmed again when dragged back this far (pt) below the threshold: no flicker, no
    /// double tick.
    public static let hysteresisDp: Float = 10

    // ---- Look of the revealed action: the whole pill enters (T3: scale 0.78 -> 1, translateX 22 -> 0, opacity 0 -> 1) ----

    /// Width (pt) of the zone that holds the action pill (the Open stage). The pill only stretches past the arming threshold.
    public static let actionsWidthDp: Float = 88
    public static let pillSizeDp: Float = 48
    public static let arrivalScaleFrom: Float = 0.78
    public static let arrivalShiftDp: Float = 22

    /// The pill arrives while the reveal goes from this share of `actionsWidthDp` to `arrivalEnd` of it.
    public static let arrivalStart: Float = 0.10
    public static let arrivalEnd: Float = 0.90

    // ---- Timings of the exit (T3: stretch about 220 ms with an ease-out cubic, then collapse about 180 ms) -------------

    public static let stretchMs = 220
    public static let collapseMs = 180

    /// The card is brought back after this delay if it is still in the list after the action (search results, failed call).
    public static let restoreDelayMs: Int64 = 450

    /// Control points of the ease-out cubic of the exit (T3: `Easing.out(Easing.cubic)`).
    public static let easeOutCubic: (x1: Float, y1: Float, x2: Float, y2: Float) = (0.33, 1, 0.68, 1)

    // Spring of the return below the threshold (T3: damping 26, stiffness 330, overshoot clamped). Mass 1, so
    // ratio = c / (2 sqrt(k)).
    public static let returnStiffness: Float = 330
    public static let returnDampingRatio: Float = 26 / (2 * Float(330).squareRoot())

    /// The pill settles without bounce on `.open` (critically damped): it must not stretch past its place.
    public static let openDampingRatio: Float = 1

    /// Minimum touch target (pt) of the revealed action.
    public static let minTouchDp: Float = 48

    /// Distance to drag from `.resting` to arm the card, for a card `width` wide.
    public static func positionalThreshold(_ width: Float) -> Float { width * thresholdFraction }

    /// Where `.open` sits: the `actionsWidthDp` zone (already converted to `actionsWidth`), so that the pill is entirely
    /// visible and centred in it. Kept under the arming threshold of a card `width` wide, so the three stages stay ordered.
    public static func openOffset(actionsWidth: Float, width: Float) -> Float {
        min(actionsWidth, positionalThreshold(width))
    }

    /// The anchors for a card of `width`: rest at 0, open at `-openOffset`, fully out to the left at `-width`. Nothing on
    /// the right. `openPx` is clamped like `openOffset`.
    public static func anchors(width: Float, openPx: Float) -> SwipeAnchors {
        SwipeAnchors(resting: 0, open: -openOffset(actionsWidth: openPx, width: width), out: -width)
    }

    /// How far the card is dragged to the left (>= 0). The offset is NaN until the anchors are known: 0 then.
    public static func reveal(_ offset: Float) -> Float { offset.isNaN ? 0 : max(-offset, 0) }

    /// The `SwipeZone` at a `reveal` of this many points, on a card of `width` whose open stage is at `openPx`. A stage is
    /// entered at its distance; once in it (or beyond), it is only left `hysteresis` below: no flicker, no double tick
    /// around a stage. Used for the visual, the haptic tick AND the decision at the release.
    public static func nextZone(was: SwipeZone, reveal: Float, width: Float, openPx: Float, hysteresis: Float) -> SwipeZone {
        if width <= 0 { return .closed }
        let threshold = positionalThreshold(width)
        let open = openOffset(actionsWidth: openPx, width: width)
        let armed = was == .armed ? reveal >= threshold - hysteresis : reveal >= threshold
        if armed { return .armed }
        let opened = was != .closed ? reveal >= open - hysteresis : reveal >= open
        return opened ? .open : .closed
    }

    /// Where the card settles when the finger is lifted: `.resting` below the open stage, `.open` from it to the
    /// threshold, `.out` (the action) past the threshold. Velocity plays no part.
    public static func settleTarget(was: SwipeZone, reveal: Float, width: Float, openPx: Float, hysteresis: Float) -> SwipeValue {
        switch nextZone(was: was, reveal: reveal, width: width, openPx: openPx, hysteresis: hysteresis) {
        case .closed: .resting
        case .open: .open
        case .armed: .out
        }
    }

    /// The pill is tappable, and a tap on the card body closes instead of opening the item, only when the card is
    /// settled open.
    public static func isOpenForTaps(settled: SwipeValue, exiting: Bool) -> Bool { settled == .open && !exiting }

    /// 0..1 progress of the pill entering (scale, shift and opacity all follow it), for a reveal of `reveal`.
    public static func arrival(reveal: Float, actionsWidth: Float) -> Float {
        let from = actionsWidth * arrivalStart
        let to = actionsWidth * arrivalEnd
        return min(max((reveal - from) / (to - from), 0), 1)
    }

    /// Scale of the whole pill for an `arrival` progress: `arrivalScaleFrom` to 1. The icon inside has no scale of its own.
    public static func pillScale(_ arrival: Float) -> Float { arrivalScaleFrom + (1 - arrivalScaleFrom) * arrival }

    /// Whether to vibrate when the drag ENTERS `zone`: only for `.armed` (the 58 % threshold is crossed, a release now
    /// runs the action), the one and only haptic feedback. Opening the pill, going back to open and closing are silent.
    /// The platform effect (a UIKit feedback generator) lives on the app side, this rule stays pure.
    public static func vibratesWhenEntering(_ zone: SwipeZone) -> Bool { zone == .armed }

    /// Extra width of the pill, the "stretch" (T3: `stretch = reveal - actionsWidth`). It STARTS as soon as the card goes
    /// past the open stage (`actionsWidth`): the pill grows to the LEFT with the finger, so its centre moves left by
    /// `stretch / 2`.
    public static func stretch(reveal: Float, actionsWidth: Float) -> Float { max(0, reveal - actionsWidth) }

    /// Where the icon sits INSIDE the stretching pill, as a `translationX` from the centre of the pill (positive = to the
    /// RIGHT), for a `reveal` on a card `width` wide. The pill grows to the left, so its centre moves left by `stretch / 2`
    /// ON SCREEN. The icon is only moved, never resized. Three segments, continuous in position at
    /// `T = thresholdFraction * width` and at `R = max(reachFraction * width, T)`:
    ///  - up to T (from rest, through the open stage): the icon is strictly CENTRED in the pill, offset 0. It does not
    ///    move relative to the pill; on screen it simply follows the centre of the pill, which stretches;
    ///  - from T to R: it takes off from the centre to the LEFT, accelerating: `offset = -stretch(R) / 2 * q`,
    ///    `q = (reveal - T) / (R - T)` in 0..1. At `reveal == R` the offset is `-stretch(R) / 2`, the centre of the
    ///    semicircle at the left end of the pill (half a pill, `pillSizeDp / 2`, from its left edge): the icon is stuck
    ///    to the left edge from `reachFraction` (75 %) of the width on. It stays inside the pill all along, because the
    ///    pill grows at least as fast as q;
    ///  - from R on: it stays stuck to the left edge of the pill, which keeps growing to the left:
    ///    `offset = -stretch(reveal) / 2` (equal to the previous segment at R, no jump). The whole icon stays inside
    ///    the pill.
    /// The position is continuous at T (offset 0 on both sides); the speed jumps from 0 to a steep slope, on purpose:
    /// the icon takes off as the strong tick plays. 0 on an unmeasured card or when the pill is not stretched yet.
    public static func iconOffsetInPill(
        reveal: Float, actionsWidth: Float, width: Float, thresholdFraction: Float = SwipeArchiveLogic.thresholdFraction
    ) -> Float {
        let currentStretch = Self.stretch(reveal: reveal, actionsWidth: actionsWidth)
        if width <= 0 || currentStretch <= 0 { return 0 }
        let threshold = width * thresholdFraction
        if reveal <= threshold { return 0 }
        let reach = max(width * reachFraction, threshold)
        if reveal >= reach { return -currentStretch / 2 }
        let q = min(max((reveal - threshold) / (reach - threshold), 0), 1)
        return -Self.stretch(reveal: reach, actionsWidth: actionsWidth) / 2 * q
    }

    /// Duration of the two exit phases, in ms: none at all with reduced motion (the card is removed at once).
    public static func exitTimings(reduceMotion: Bool) -> (stretchMs: Int, collapseMs: Int) {
        reduceMotion ? (0, 0) : (stretchMs, collapseMs)
    }
}

/// The action decided at the release, delivered exactly once: after the exit animation, or on disposal if the card leaves
/// the screen first.
public final class PendingDelivery {
    private var pending: ArchiveAction?

    public init() {}

    public func arm(_ action: ArchiveAction) {
        pending = action
    }

    /// Hands the action to `sink` if there is one not delivered yet. Returns whether it did.
    @discardableResult
    public func deliver(_ sink: (ArchiveAction) -> Void) -> Bool {
        guard let action = pending else { return false }
        pending = nil
        sink(action)
        return true
    }
}
