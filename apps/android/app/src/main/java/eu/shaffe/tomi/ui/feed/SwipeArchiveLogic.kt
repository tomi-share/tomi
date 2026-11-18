package eu.shaffe.tomi.ui.feed

import androidx.compose.animation.core.AnimationSpec
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.AnchoredDraggableState
import androidx.compose.foundation.gestures.DraggableAnchors
import androidx.compose.foundation.gestures.FlingBehavior
import androidx.compose.foundation.gestures.ScrollScope
import kotlin.math.max

/**
 * The three resting places of a card that is swiped to reveal its action (two steps, as in iOS Mail / Gmail):
 *  - [Resting]: offset 0, closed;
 *  - [Open]: offset `-ACTIONS_WIDTH` (88 dp), the pill is fully revealed, settled and tappable, nothing is archived yet;
 *  - [Out]: offset `-width`, the card is fully out to the LEFT = the action is committed (archive / un-archive).
 * There is NO anchor on the right, so a drag to the right can not move the card past [Resting].
 */
enum class SwipeValue { Resting, Open, Out }

/**
 * Where the finger (or the card) is, by distance dragged: [Closed] (the pill has not fully entered), [Open] (the pill is fully
 * there), [Armed] (past 58 % of the width: releasing now runs the action). It drives the visual, the haptic tick and the decision
 * at the release, so that what the user feels is what happens.
 */
enum class SwipeZone { Closed, Open, Armed }

/** Ease-out cubic (T3: `Easing.out(Easing.cubic)`). */
internal val EaseOutCubic = CubicBezierEasing(0.33f, 1f, 0.68f, 1f)

/**
 * Pure rules of the swipe-to-archive, free of any gesture code so that they can be unit tested. The gesture itself
 * (axis lock, touch slop, conflict with the vertical scroll of the list) is `Modifier.anchoredDraggable(Orientation.Horizontal)`,
 * from Compose foundation. The behaviour and the timings are the ones of the reference app (T3 Code, `thread-swipe-actions`):
 *
 *  - the decision is taken WHEN THE FINGER IS LIFTED, by how far the card was dragged, never by the velocity:
 *    below the opening stage ([ACTIONS_WIDTH_DP], the pill is not fully in) the card closes; from the opening stage to
 *    [THRESHOLD_FRACTION] of the width it stays [SwipeValue.Open] (the pill is tappable); past the threshold it is "armed" and the
 *    release runs the action. Crossing the threshold while dragging only gives a haptic tick (the only one), it never runs anything;
 *  - the threshold is [THRESHOLD_FRACTION] of the card width (T3: `0.58`);
 *  - from [SwipeValue.Open] a tap on the pill runs the same action as the full swipe, a tap on the card or a drag to the right closes it.
 */
object SwipeArchiveLogic {
    /** Share of the card width to drag before the release runs the action (T3: 0.58). */
    const val THRESHOLD_FRACTION = 0.58f

    /** Share of the card width from which the icon is stuck to the left edge of the stretching pill (before, it travels there from 58 %). */
    const val REACH_FRACTION = 0.75f

    /** Once armed, the card is only disarmed again when dragged back this far (dp) below the threshold: no flicker, no double tick. */
    const val HYSTERESIS_DP = 10f

    // ---- Look of the revealed action: the whole pill enters (T3: scale 0.78 -> 1, translateX 22 -> 0, opacity 0 -> 1) -----------

    /** Width (dp) of the zone that holds the action pill (the Open stage). The pill only stretches past the arming threshold. */
    const val ACTIONS_WIDTH_DP = 88f
    const val PILL_SIZE_DP = 48f
    const val ARRIVAL_SCALE_FROM = 0.78f
    const val ARRIVAL_SHIFT_DP = 22f

    /** The pill arrives while the reveal goes from this share of [ACTIONS_WIDTH_DP] to [ARRIVAL_END] of it. */
    const val ARRIVAL_START = 0.10f
    const val ARRIVAL_END = 0.90f

    // ---- Timings of the exit (T3: stretch about 220 ms with an ease-out cubic, then collapse about 180 ms) --------------------

    const val STRETCH_MS = 220
    const val COLLAPSE_MS = 180

    /** The card is brought back after this delay if it is still in the list after the action (search results, failed call). */
    const val RESTORE_DELAY_MS = 450L

    // Spring of the return below the threshold (T3: damping 26, stiffness 330, overshoot clamped). Mass 1, so ratio = c / (2 sqrt(k)).
    const val RETURN_STIFFNESS = 330f
    val RETURN_DAMPING_RATIO: Float = 26f / (2f * kotlin.math.sqrt(RETURN_STIFFNESS))

    /** The pill settles without bounce on [SwipeValue.Open] (critically damped): it must not stretch past its place. */
    const val OPEN_DAMPING_RATIO = 1f

    /** Minimum touch target (dp) of the revealed action. */
    const val MIN_TOUCH_DP = 48f

    /** Distance to drag from [SwipeValue.Resting] to arm the card, for a card [width] px wide. */
    fun positionalThreshold(width: Float): Float = width * THRESHOLD_FRACTION

    /**
     * Where [SwipeValue.Open] sits, px: the [ACTIONS_WIDTH_DP] zone (already converted to [actionsWidth] px), so that the pill is
     * entirely visible and centred in it. Kept under the arming threshold of a card [width] px wide, so the three stages stay ordered.
     */
    fun openOffset(actionsWidth: Float, width: Float): Float = actionsWidth.coerceAtMost(positionalThreshold(width))

    /**
     * The anchors for a card of [width] px: rest at 0, open at `-openPx`, fully out to the left at `-width`. Nothing on the right.
     * [openPx] is clamped like [openOffset].
     */
    fun anchors(width: Float, openPx: Float): DraggableAnchors<SwipeValue> = DraggableAnchors {
        SwipeValue.Resting at 0f
        SwipeValue.Open at -openOffset(openPx, width)
        SwipeValue.Out at -width
    }

    /** A new state, at rest. Never restored from a saved state: a restored position would bring a card back already swiped. */
    fun newState(): AnchoredDraggableState<SwipeValue> = AnchoredDraggableState(SwipeValue.Resting)

    /** How far the card is dragged to the left, px (>= 0). The state offset is NaN until the anchors are known: 0 then. */
    fun reveal(offset: Float): Float = if (offset.isNaN()) 0f else max(-offset, 0f)

    /**
     * The [SwipeZone] at a [reveal] of this many px, on a card of [width] px whose open stage is at [openPx]. A stage is entered
     * at its distance; once in it (or beyond), it is only left [hysteresis] px below: no flicker, no double tick around a stage.
     * Used for the visual, the haptic tick AND the decision at the release.
     */
    fun nextZone(was: SwipeZone, reveal: Float, width: Float, openPx: Float, hysteresis: Float): SwipeZone {
        if (width <= 0f) return SwipeZone.Closed
        val threshold = positionalThreshold(width)
        val open = openOffset(openPx, width)
        val armed = if (was == SwipeZone.Armed) reveal >= threshold - hysteresis else reveal >= threshold
        if (armed) return SwipeZone.Armed
        val opened = if (was != SwipeZone.Closed) reveal >= open - hysteresis else reveal >= open
        return if (opened) SwipeZone.Open else SwipeZone.Closed
    }

    /**
     * Where the card settles when the finger is lifted: [SwipeValue.Resting] below the open stage, [SwipeValue.Open] from it to
     * the threshold, [SwipeValue.Out] (the action) past the threshold. Velocity plays no part.
     */
    fun settleTarget(was: SwipeZone, reveal: Float, width: Float, openPx: Float, hysteresis: Float): SwipeValue =
        when (nextZone(was, reveal, width, openPx, hysteresis)) {
            SwipeZone.Closed -> SwipeValue.Resting
            SwipeZone.Open -> SwipeValue.Open
            SwipeZone.Armed -> SwipeValue.Out
        }

    /** The pill is tappable, and a tap on the card body closes instead of opening the item, only when the card is settled open. */
    fun isOpenForTaps(settled: SwipeValue, exiting: Boolean): Boolean = settled == SwipeValue.Open && !exiting

    /** 0..1 progress of the pill entering (scale, shift and opacity all follow it), for a reveal of [reveal] px. */
    fun arrival(reveal: Float, actionsWidth: Float): Float {
        val from = actionsWidth * ARRIVAL_START
        val to = actionsWidth * ARRIVAL_END
        return ((reveal - from) / (to - from)).coerceIn(0f, 1f)
    }

    /** Scale of the whole pill for an [arrival] progress: [ARRIVAL_SCALE_FROM] to 1. The icon inside has no scale of its own. */
    fun pillScale(arrival: Float): Float = ARRIVAL_SCALE_FROM + (1f - ARRIVAL_SCALE_FROM) * arrival

    /**
     * Whether to vibrate when the drag ENTERS [zone]: only for [SwipeZone.Armed] (the 58 % threshold is crossed, a release now runs the
     * action), the one and only haptic feedback. Opening the pill, going back to open and closing are silent. The platform effect
     * (`EFFECT_HEAVY_CLICK`) lives on the View / Context side (`SwipeToArchive.kt`), this rule stays pure.
     */
    fun vibratesWhenEntering(zone: SwipeZone): Boolean = zone == SwipeZone.Armed

    /**
     * Extra width (px) of the pill, the "stretch" (T3: `stretch = reveal - actionsWidth`). It STARTS as soon as the card goes past the
     * open stage ([actionsWidth] px): the pill grows to the LEFT with the finger, so its centre moves left by `stretch / 2`.
     */
    fun stretch(reveal: Float, actionsWidth: Float): Float = max(0f, reveal - actionsWidth)

    /**
     * Where the icon sits INSIDE the stretching pill, px, as a `translationX` from the centre of the pill (positive = to the RIGHT),
     * for a [reveal] on a card [width] px wide. The pill grows to the left, so its centre moves left by `stretch / 2` ON SCREEN. The
     * icon is only moved, never resized. Three segments, continuous in position at `T = thresholdFraction * width` and at
     * `R = max(REACH_FRACTION * width, T)`:
     *  - up to T (from rest, through the open stage): the icon is strictly CENTRED in the pill, offset 0. It does not move relative
     *    to the pill; on screen it simply follows the centre of the pill, which stretches;
     *  - from T to R: it takes off from the centre to the LEFT, accelerating: `offset = -stretch(R) / 2 * q`,
     *    `q = (reveal - T) / (R - T)` in 0..1. At `reveal == R` the offset is `-stretch(R) / 2`, the centre of the semicircle at the
     *    left end of the pill (half a pill, [PILL_SIZE_DP] / 2, from its left edge): the icon is stuck to the left edge from
     *    [REACH_FRACTION] (75 %) of the width on. It stays inside the pill all along, because the pill grows at least as fast as q;
     *  - from R on: it stays stuck to the left edge of the pill, which keeps growing to the left: `offset = -stretch(reveal) / 2`
     *    (equal to the previous segment at R, no jump). The whole icon stays inside the pill.
     * The position is continuous at T (offset 0 on both sides); the speed jumps from 0 to a steep slope, on purpose: the icon takes off
     * as the strong tick plays. 0 on an unmeasured card or when the pill is not stretched yet.
     */
    fun iconOffsetInPill(reveal: Float, actionsWidth: Float, width: Float, thresholdFraction: Float = THRESHOLD_FRACTION): Float {
        val stretch = stretch(reveal, actionsWidth)
        if (width <= 0f || stretch <= 0f) return 0f
        val threshold = width * thresholdFraction
        if (reveal <= threshold) return 0f
        val reach = max(width * REACH_FRACTION, threshold)
        if (reveal >= reach) return -stretch / 2f
        val q = ((reveal - threshold) / (reach - threshold)).coerceIn(0f, 1f)
        return -stretch(reach, actionsWidth) / 2f * q
    }

    /** Duration of the two exit phases, in ms: none at all with reduced motion (the card is removed at once). */
    fun exitTimings(reduceMotion: Boolean): Pair<Int, Int> = if (reduceMotion) 0 to 0 else STRETCH_MS to COLLAPSE_MS
}

/**
 * Settles the card when the finger is lifted (`Modifier.anchoredDraggable(flingBehavior = ...)`). The default behaviour of
 * `AnchoredDraggable` decides with the release velocity: any flick above 125 dp/s goes to the next anchor wherever the finger
 * is, and a release at rest picks the closest anchor (50 %). Both contradict the stage shown to the user (open pill, 58 %,
 * haptic ticks). Here the decision is [targetFor] only, i.e. the zone, and the card is animated there: out with the stretch of the
 * exit, on the open stage or back to rest with a spring. With [reduceMotion] it jumps.
 */
internal class ArchiveSettleFling(
    private val state: AnchoredDraggableState<SwipeValue>,
    private val targetFor: () -> SwipeValue,
    private val reduceMotion: Boolean,
) : FlingBehavior {
    override suspend fun ScrollScope.performFling(initialVelocity: Float): Float {
        val start = state.requireOffset()
        val value = targetFor()
        val target = state.anchors.positionOf(value)
        if (start == target) return 0f
        if (reduceMotion) {
            scrollBy(target - start)
            return 0f
        }
        val spec: AnimationSpec<Float> = when (value) {
            SwipeValue.Out -> tween(SwipeArchiveLogic.STRETCH_MS, easing = EaseOutCubic)
            // Open: critically damped, so the pill never swells past its place.
            SwipeValue.Open -> spring(SwipeArchiveLogic.OPEN_DAMPING_RATIO, SwipeArchiveLogic.RETURN_STIFFNESS)
            SwipeValue.Resting -> spring(SwipeArchiveLogic.RETURN_DAMPING_RATIO, SwipeArchiveLogic.RETURN_STIFFNESS)
        }
        // Keep the finger's momentum only when it points to the target, and never for Open (it would overshoot the pill's place).
        val velocity = if (value != SwipeValue.Open && (target - start) * initialVelocity > 0f) initialVelocity else 0f
        animate(start, target, velocity, spec) { v, _ -> scrollBy(v - state.requireOffset()) } // the anchors clamp any overshoot
        scrollBy(target - state.requireOffset()) // land exactly on the anchor, so that the state settles on it
        return 0f
    }
}

/**
 * The action decided at the release, delivered exactly once: after the exit animation, or on disposal if the card leaves the
 * screen first.
 */
class PendingDelivery {
    private var pending: ArchiveAction? = null

    fun arm(action: ArchiveAction) {
        pending = action
    }

    /** Hands the action to [sink] if there is one not delivered yet. Returns whether it did. */
    fun deliver(sink: (ArchiveAction) -> Unit): Boolean {
        val action = pending ?: return false
        pending = null
        sink(action)
        return true
    }
}
