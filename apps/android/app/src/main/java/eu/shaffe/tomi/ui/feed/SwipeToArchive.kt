package eu.shaffe.tomi.ui.feed

import android.content.Context
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.indication
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.anchoredDraggable
import androidx.compose.foundation.gestures.animateTo
import androidx.compose.foundation.gestures.snapTo
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.Unarchive
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.ui.theme.TomiTheme
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/**
 * The system vibrator (`VibratorManager.defaultVibrator`, API 31+; the minSdk is 33), or null if the service is missing.
 * `View.performHapticFeedback` is NOT used: it follows the system "touch feedback" setting (the flag that ignores it is reserved to
 * privileged apps since API 31) and stayed silent on one Android phone. Calling the vibrator directly needs only the normal
 * `VIBRATE` permission (no runtime prompt).
 */
private fun systemVibrator(context: Context): Vibrator? =
    context.getSystemService(VibratorManager::class.java)?.defaultVibrator

/**
 * The one haptic tick of the swipe, through the vibrator, FORCED on purpose (the reference app T3 Code vibrates whatever the phone
 * mode): no check of any system setting, only [Vibrator.hasVibrator] (a tablet without motor must not crash). PREDEFINED effect,
 * which One UI renders better than a hand-made waveform: `EFFECT_HEAVY_CLICK`. The platform falls back to a generic waveform when the
 * hardware has no such effect. `vibrate(effect)` is called without `VibrationAttributes`: the default (unknown usage) is not subject
 * to the touch-feedback setting.
 */
private fun performStrongTick(vibrator: Vibrator?) {
    if (vibrator == null || !vibrator.hasVibrator()) return
    vibrator.vibrate(VibrationEffect.createPredefined(VibrationEffect.EFFECT_HEAVY_CLICK))
}
/** True when the system animation scale is 0 ("remove animations"): no animation, the row is removed at once. */
private fun isReducedMotion(context: Context): Boolean =
    Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f

/**
 * Swipe to the LEFT on a card = [action] (archive, or un-archive on an archived card), in TWO steps (swipe to reveal, as iOS Mail /
 * Gmail / the reference app T3 Code `thread-swipe-actions`). Never a deletion, never another direction, no confirmation: the feedback
 * banner offers "Undo". With a null [action] the card does not swipe.
 *
 * Gesture: `Modifier.anchoredDraggable(Orientation.Horizontal)` on an `AnchoredDraggableState<SwipeValue>` with three anchors,
 * [SwipeValue.Resting] (0), [SwipeValue.Open] (`-88 dp`, the pill fully revealed) and [SwipeValue.Out] (`-width`, the card fully out).
 * There is no anchor on the right: a drag to the right never moves the card past rest. The drag detection only looks at the
 * horizontal axis (touch slop from the view configuration), so a vertical drift of the finger never claims the gesture and the
 * `LazyColumn` keeps its vertical scroll: this is the standard conflict resolution of the pointer system (the child that first passes
 * its slop on its own axis wins). Taps and long presses are untouched. A child that scrolls sideways and sits under the finger wins the
 * horizontal drags that start on it (child before parent): the feed cards have no such child.
 *
 * Decision when the finger is lifted ([ArchiveSettleFling]), by distance only, never by velocity:
 *  - pill not fully entered (reveal < 88 dp): the card closes ([SwipeValue.Resting]), nothing happens;
 *  - from 88 dp to 58 % of the width: the card stays [SwipeValue.Open], pill revealed and tappable, nothing is archived;
 *  - from 58 % of the width: the card goes [SwipeValue.Out] and the action runs (the full-swipe behaviour).
 *
 * From Open: a tap on the pill runs the same action through the same path (card out + stretch, row collapse, [onAction] once, banner
 * "Undo"); a tap on the card body, a drag to the right or a scroll of the list closes it WITHOUT opening the item (the body tap is
 * consumed by a transparent layer while the card is open); a drag to the left past 58 % then a release runs the action.
 * [keepOpen] / [onOpenChange] let the list keep a single card open: the card reports whether it is out of rest and closes itself when
 * [keepOpen] turns false.
 *
 * [onAction] is called ONLY once the state has settled on [SwipeValue.Out] and the row has collapsed (at once with reduced motion),
 * never during the drag. Haptics: a single strong tick when 58 % is crossed (a release now archives); nothing when the pill opens,
 * goes back or closes, and nothing during the exit, the restore, or a programmatic close.
 *
 * Look: under the card, `color.*.brand-container` with the `archive` / `unarchive` icon (a box, not a circular arrow, charter 6.3)
 * in a `brand` pill. The whole pill enters from the right with a zoom (scale .78 to 1, shift 22 dp to 0, opacity 0 to 1), the icon
 * keeps its size; at Open it sits centred in its 88 dp zone; past it the pill stretches with the finger and the icon stays
 * centred in the stretching pill up to 58 %; past 58 % it takes off to the left,
 * faster, and is stuck to the left end of the pill from 75 % on. On validation the card slides
 * out while the pill stretches (220 ms, ease-out cubic), then the row collapses in height (180 ms). With "remove animations" there is
 * no animation: the row is removed at once.
 */
@Composable
fun SwipeToArchive(
    action: ArchiveAction?,
    onAction: (ArchiveAction) -> Unit,
    modifier: Modifier = Modifier,
    keepOpen: Boolean = true,
    onOpenChange: (Boolean) -> Unit = {},
    content: @Composable () -> Unit,
) {
    val currentAction by rememberUpdatedState(action)
    val currentOnAction by rememberUpdatedState(onAction)
    val currentOnOpenChange by rememberUpdatedState(onOpenChange)
    val context = LocalContext.current
    val vibrator = remember(context) { systemVibrator(context) }
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()
    val reduceMotion = isReducedMotion(context)
    val hysteresisPx = with(density) { SwipeArchiveLogic.HYSTERESIS_DP.dp.toPx() }
    val actionsWidthPx = with(density) { SwipeArchiveLogic.ACTIONS_WIDTH_DP.dp.toPx() }

    // A plain `remember`, NOT `rememberSaveable`: a card that comes back (Undo, new composition) starts closed, never swiped or open.
    val state = remember { SwipeArchiveLogic.newState() }
    val delivery = remember { PendingDelivery() }
    /** 0 = full height, 1 = collapsed. */
    val collapse = remember { Animatable(0f) }
    var widthPx by remember { mutableFloatStateOf(0f) }
    var zone by remember { mutableStateOf(SwipeZone.Closed) }
    /** True from the commit (settle or tap on the pill) until the exit (and a possible restore) is over: no new drag meanwhile. */
    var exiting by remember { mutableStateOf(false) }
    /** True while the card is closed by a tap / by the list (no gesture, so no haptic tick). */
    var closing by remember { mutableStateOf(false) }

    // The action is delivered once: here, or on disposal if the card leaves the composition before the end of its exit.
    fun deliver() {
        delivery.deliver { currentOnAction(it) }
    }

    DisposableEffect(Unit) {
        onDispose { deliver() }
    }

    /** Closes an open card (tap on its body, list scroll, another card opening). */
    fun close() {
        if (closing || exiting) return
        scope.launch {
            closing = true
            try {
                if (reduceMotion) {
                    state.snapTo(SwipeValue.Resting)
                } else {
                    state.animateTo(SwipeValue.Resting, spring(SwipeArchiveLogic.RETURN_DAMPING_RATIO, SwipeArchiveLogic.RETURN_STIFFNESS))
                }
            } finally {
                closing = false
            }
        }
    }

    /** Tap on the pill: the same commit as the full swipe, i.e. the card goes out and the settle on Out runs the exit below. */
    fun commitFromPill() {
        if (currentAction == null || exiting) return
        exiting = true // no drag, no tick from now on
        scope.launch {
            if (reduceMotion) {
                state.snapTo(SwipeValue.Out)
            } else {
                state.animateTo(SwipeValue.Out, tween(SwipeArchiveLogic.STRETCH_MS, easing = EaseOutCubic))
            }
            if (state.settledValue != SwipeValue.Out) exiting = false // interrupted before the commit: the card is usable again
        }
    }

    // The anchors depend on the width of the card, known after the first layout.
    LaunchedEffect(state, widthPx, actionsWidthPx) {
        if (widthPx > 0f) state.updateAnchors(SwipeArchiveLogic.anchors(widthPx, actionsWidthPx))
    }

    // Stages: pill opening, armed past 58 %, and back. One strong haptic tick per crossing of 58 % (hysteresis in nextZone).
    LaunchedEffect(state) {
        snapshotFlow { SwipeArchiveLogic.reveal(state.offset) }.collect { reveal ->
            val next = SwipeArchiveLogic.nextZone(zone, reveal, widthPx, actionsWidthPx, hysteresisPx)
            if (next == zone) return@collect
            zone = next
            // the card going back after a restore, or closed by a tap, is not a gesture: no tick
            if (!exiting && !closing && SwipeArchiveLogic.vibratesWhenEntering(next)) performStrongTick(vibrator)
        }
    }

    // The list tells whether this card may stay open (a single one at a time).
    LaunchedEffect(zone) { currentOnOpenChange(zone != SwipeZone.Closed) }
    LaunchedEffect(keepOpen) {
        if (!keepOpen && zone != SwipeZone.Closed && !exiting) close()
    }

    // The trigger: the state has SETTLED on Out (finger lifted while armed, or tap on the pill). Collapse the row, then run the action.
    LaunchedEffect(state, reduceMotion) {
        snapshotFlow { state.settledValue }.collect { value ->
            if (value != SwipeValue.Out) return@collect
            val decided = currentAction
            if (decided == null) {
                state.snapTo(SwipeValue.Resting)
                exiting = false
                return@collect
            }
            val (_, collapseMs) = SwipeArchiveLogic.exitTimings(reduceMotion)
            exiting = true
            delivery.arm(decided)
            try {
                // Collapse: the row folds up in height (the card is already out and the pill stretched by the settle).
                if (!reduceMotion) collapse.animateTo(1f, tween(collapseMs, easing = FastOutSlowInEasing))
                deliver()
                // Still here a moment after the action (search results, failed call): bring the row back instead of leaving a hole.
                delay(SwipeArchiveLogic.RESTORE_DELAY_MS)
                if (reduceMotion) {
                    collapse.snapTo(0f)
                    state.snapTo(SwipeValue.Resting)
                } else {
                    collapse.animateTo(0f, tween(collapseMs, easing = FastOutSlowInEasing))
                    state.animateTo(SwipeValue.Resting, tween(SwipeArchiveLogic.STRETCH_MS, easing = EaseOutCubic))
                }
                zone = SwipeZone.Closed
            } finally {
                exiting = false
            }
        }
    }

    val label = action?.let { stringResource(if (it == ArchiveAction.Archive) R.string.action_archive else R.string.action_unarchive) }
    val showBackground by remember { derivedStateOf { SwipeArchiveLogic.reveal(state.offset) > 0f } }
    /** Settled open: the pill is tappable and the body tap closes instead of opening the item. */
    val isOpen by remember { derivedStateOf { SwipeArchiveLogic.isOpenForTaps(state.settledValue, exiting) } }
    val fling = remember(state, reduceMotion, hysteresisPx, actionsWidthPx) {
        ArchiveSettleFling(
            state = state,
            targetFor = { SwipeArchiveLogic.settleTarget(zone, SwipeArchiveLogic.reveal(state.offset), widthPx, actionsWidthPx, hysteresisPx) },
            reduceMotion = reduceMotion,
        )
    }

    Box(
        modifier
            .graphicsLayer { clip = collapse.value > 0f }
            .layout { measurable, constraints ->
                val placeable = measurable.measure(constraints)
                val height = (placeable.height * (1f - collapse.value.coerceIn(0f, 1f))).roundToInt()
                layout(placeable.width, height) { placeable.placeRelative(0, 0) }
            }
            .onSizeChanged { widthPx = it.width.toFloat() }
            .anchoredDraggable(
                state = state,
                orientation = Orientation.Horizontal,
                // Not before the anchors are known (the offset is NaN until then), not during the exit, not without an action.
                enabled = action != null && !exiting && !state.offset.isNaN(),
                flingBehavior = fling,
            )
            .then(
                if (action != null && label != null) {
                    // TalkBack cannot swipe: the same action as a custom action of the card (no gesture, no animation).
                    Modifier.semantics { customActions = listOf(CustomAccessibilityAction(label) { onAction(action); true }) }
                } else {
                    Modifier
                },
            ),
    ) {
        // Nothing under the card at rest: no coloured fringe around its border.
        if (showBackground && action != null && label != null) {
            Box(
                Modifier
                    .matchParentSize()
                    .clip(TomiTheme.shapes.card)
                    .background(MaterialTheme.colorScheme.primaryContainer),
            ) {
                ActionPill(
                    unarchive = action == ArchiveAction.Unarchive,
                    label = label,
                    enabled = isOpen,
                    onClick = ::commitFromPill,
                    reveal = { SwipeArchiveLogic.reveal(state.offset) },
                    cardWidth = { widthPx },
                )
            }
        }
        Box(Modifier.graphicsLayer { translationX = -SwipeArchiveLogic.reveal(state.offset) }) {
            content()
            if (isOpen) {
                // Reveal convention: while the card is open, a tap on its body closes it instead of opening the item. This layer sits
                // above the content, so the content never sees the tap; drags pass through it (it only consumes the tap).
                Box(
                    Modifier
                        .matchParentSize()
                        .pointerInput(reduceMotion) { detectTapGestures(onTap = { close() }) },
                )
            }
        }
    }
}

/**
 * The `brand` pill with the action icon, in the zone revealed under the card. The whole pill enters from the right with a zoom and
 * a fade; at the open stage it is centred in the [SwipeArchiveLogic.ACTIONS_WIDTH_DP] zone. Past that zone the pill stretches to
 * the LEFT with the finger (its centre moves left by half the stretch). The icon is carried by [SwipeArchiveLogic.iconOffsetInPill]
 * (signed `translationX` from the centre of the pill): it stays centred in the pill up to the arming threshold (58 %), then takes off to the left, accelerating, and is stuck to the left end of the pill from 75 % ([SwipeArchiveLogic.REACH_FRACTION]). Opaque, never resized; [cardWidth] is the width of the
 * card in px. The whole zone (88 dp wide, the height of the card, at least 48 dp) is the
 * touch target of a button ([label] for the accessibility services), enabled only once the card is settled open ([enabled]); the
 * ripple is drawn on the pill.
 */
@Composable
private fun ActionPill(
    unarchive: Boolean,
    label: String,
    enabled: Boolean,
    onClick: () -> Unit,
    reveal: () -> Float,
    cardWidth: () -> Float,
) {
    val density = LocalDensity.current
    val actionsWidth = with(density) { SwipeArchiveLogic.ACTIONS_WIDTH_DP.dp.toPx() }
    val pill = with(density) { SwipeArchiveLogic.PILL_SIZE_DP.dp.roundToPx() }
    val shift = with(density) { SwipeArchiveLogic.ARRIVAL_SHIFT_DP.dp.toPx() }
    val interaction = remember { MutableInteractionSource() }
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.CenterEnd) {
        // The touch target, under the pill (the pill has no pointer input, so presses reach it).
        Box(
            Modifier
                .width(SwipeArchiveLogic.ACTIONS_WIDTH_DP.dp)
                .fillMaxHeight()
                .heightIn(min = SwipeArchiveLogic.MIN_TOUCH_DP.dp)
                .clickable(
                    interactionSource = interaction,
                    indication = null,
                    enabled = enabled,
                    onClickLabel = label,
                    role = Role.Button,
                    onClick = onClick,
                )
                .semantics { contentDescription = label },
        )
        Box(
            Modifier.fillMaxSize().padding(end = ((SwipeArchiveLogic.ACTIONS_WIDTH_DP - SwipeArchiveLogic.PILL_SIZE_DP) / 2f).dp),
            contentAlignment = Alignment.CenterEnd,
        ) {
            Box(
                Modifier
                    .layout { measurable, _ ->
                        val width = pill + SwipeArchiveLogic.stretch(reveal(), actionsWidth).roundToInt()
                        val placeable = measurable.measure(Constraints.fixed(width, pill))
                        layout(width, pill) { placeable.placeRelative(0, 0) }
                    }
                    // The WHOLE pill makes the entrance (zoom, shift from the right, fade); the icon inside is fixed and opaque.
                    .graphicsLayer {
                        val arrival = SwipeArchiveLogic.arrival(reveal(), actionsWidth)
                        val scale = SwipeArchiveLogic.pillScale(arrival)
                        scaleX = scale
                        scaleY = scale
                        translationX = shift * (1f - arrival)
                        alpha = arrival
                    }
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.primary)
                    .indication(interaction, ripple(color = MaterialTheme.colorScheme.onPrimary)),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    if (unarchive) Icons.Outlined.Unarchive else Icons.Outlined.Archive,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onPrimary,
                    // Centred in the pill up to the threshold, then takes off to the left (no scale).
                    modifier = Modifier.graphicsLayer {
                        translationX = SwipeArchiveLogic.iconOffsetInPill(reveal(), actionsWidth, cardWidth())
                    },
                )
            }
        }
    }
}
