package eu.shaffe.tomi.ui.render

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.Constraints
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Items composed at once are those that fill the first screen, by their estimated height: usually one (an item is about two
 * screens tall), so that opening the detail costs one rendering; a short first item (a title before a long table) takes the
 * next ones with it instead of leaving the screen blank until it has landed. Never more than this many.
 */
private const val MAX_INITIAL_ITEMS = 4

/** The next item is revealed while the end of the revealed ones is less than this many screens below the bottom of the window. */
private const val LOOKAHEAD_SCREENS = 1.5f

/** An item that still has no height after this long (empty rendering, lost layout request) no longer holds the next ones back. */
private const val SETTLE_TIMEOUT_MS = 600L

/** Height of one unit of weight (dp), to reserve room before anything was measured: see [textWeight]. */
private const val ESTIMATED_DP_PER_WEIGHT = 0.5f

/** What a line break weighs, in characters: a line is at least one line high, however short. */
private const val LINE_WEIGHT = 20

/**
 * Weight of the text [start, end) of [text], for the height estimates of [ProgressiveColumn]: its characters (about 40 a line
 * of body text, 22 dp high) plus [LINE_WEIGHT] per line break.
 */
internal fun textWeight(text: CharSequence, start: Int = 0, end: Int = text.length): Int {
    var lines = 0
    for (i in start until end) if (text[i] == '\n') lines++
    return (end - start) + LINE_WEIGHT * lines
}

/** What the layout of a [ProgressiveColumn] tells the coroutine that reveals its items. Written at layout, never read there. */
private class ProgressState(count: Int) {
    /** Laid out height of each revealed item (0: not laid out yet, or nothing to show). */
    val heights = mutableStateListOf<Int>().apply { repeat(count) { add(0) } }

    /** The same heights, to write [heights] only on a change without reading it (a read would lay out again). */
    private val last = IntArray(count)

    /** Revealed items below this index are settled: laid out, or given up on (an item may legitimately render to nothing). */
    var settled by mutableIntStateOf(0)

    /** Top of the column in the window, and height of its revealed items: their end is where the next item will come. */
    var top by mutableFloatStateOf(Float.POSITIVE_INFINITY)
    var contentHeight by mutableIntStateOf(0)

    fun report(index: Int, height: Int) {
        if (last[index] != height) {
            last[index] = height
            heights[index] = height
        }
    }
}

/**
 * A column of [count] items revealed progressively, for the detail screen (a `Column` in a `verticalScroll`, where a lazy list
 * cannot be nested). The items of the first screen are composed at once ([initialItems]); then one more each time the end of the
 * revealed items is less than [LOOKAHEAD_SCREENS] screens below the window AND the items revealed so far have been laid out with
 * a height (their rendering, asynchronous for the Markdown views, has landed): renders are never stacked. No progress indicator
 * (charter).
 *
 * The column is as tall as its revealed items plus a blank space for what is not on screen yet: the items not revealed, and the
 * revealed ones whose rendering has not landed. That space is their [weight] ([textWeight]) times the height per weight
 * measured so far, or an estimate before the first measure. It is computed in the very layout pass that measures the items, so
 * an item that lands takes its place in one step: what follows the column (the metadata of the detail) is not shown at the top
 * and then pushed down, and the scroll extent does not leap as the items come.
 *
 * [resetKey] identifies the content: a new one starts over. Only the NUMBER of revealed items is saved (rotation keeps the
 * reading position), never any content.
 */
@Composable
fun ProgressiveColumn(
    count: Int,
    weight: (Int) -> Int,
    resetKey: Any,
    modifier: Modifier = Modifier,
    item: @Composable (index: Int) -> Unit,
) {
    val windowHeight = LocalWindowInfo.current.containerSize.height
    val estimatedPxPerWeight = ESTIMATED_DP_PER_WEIGHT * LocalDensity.current.density
    var revealed by rememberSaveable(resetKey, count) {
        mutableIntStateOf(initialItems(count, weight, screenWeight = windowHeight / estimatedPxPerWeight))
    }
    val state = remember(resetKey, count) { ProgressState(count) }
    val shown = revealed.coerceIn(0, count)

    Layout(
        content = {
            // One box per item: one thing to measure, whatever the item emits.
            for (index in 0 until shown) {
                key(index) { Box(Modifier.fillMaxWidth()) { item(index) } }
            }
        },
        modifier = modifier.onGloballyPositioned { state.top = it.positionInWindow().y },
    ) { measurables, constraints ->
        val itemConstraints = Constraints(minWidth = constraints.minWidth, maxWidth = constraints.maxWidth)
        val placeables = measurables.map { it.measure(itemConstraints) }
        val settled = state.settled
        var contentHeight = 0
        var measuredPx = 0L
        var measuredWeight = 0L
        var pendingWeight = 0L
        for (index in 0 until count) {
            val itemWeight = weight(index).coerceAtLeast(1)
            val height = if (index < placeables.size) placeables[index].height else 0
            contentHeight += height
            when {
                height > 0 -> {
                    measuredPx += height
                    measuredWeight += itemWeight
                }
                index >= settled -> pendingWeight += itemWeight
            }
        }
        val pxPerWeight = if (measuredWeight > 0) measuredPx.toFloat() / measuredWeight else estimatedPxPerWeight
        val reserved = (pendingWeight * pxPerWeight).toLong().coerceIn(0L, MAX_RESERVED_PX).toInt()
        for (index in placeables.indices) state.report(index, placeables[index].height)
        state.contentHeight = contentHeight
        val width = if (constraints.hasBoundedWidth) constraints.maxWidth else placeables.maxOfOrNull { it.width } ?: constraints.minWidth
        layout(width, (contentHeight + reserved).coerceIn(constraints.minHeight, constraints.maxHeight)) {
            var y = 0
            for (placeable in placeables) {
                placeable.place(0, y)
                y += placeable.height
            }
        }
    }

    LaunchedEffect(resetKey, count, windowHeight) {
        val limit = windowHeight * (1f + LOOKAHEAD_SCREENS)
        while (true) {
            val current = revealed.coerceIn(0, count)
            // Only the items revealed since the last round: one that was given up on must not make every round wait again.
            val from = state.settled.coerceIn(0, current)
            withTimeoutOrNull(SETTLE_TIMEOUT_MS) {
                snapshotFlow { (from until current).all { state.heights[it] > 0 } }.first { it }
            }
            state.settled = current
            if (current >= count) break
            snapshotFlow { state.top + state.contentHeight }.first { it < limit }
            revealed = current + 1
        }
    }
}

/**
 * How many of the [count] items are composed at once: the first one, then the next ones while the estimated height of those
 * before them does not fill a screen ([screenWeight] = the weight a screen holds), [MAX_INITIAL_ITEMS] at most.
 */
internal fun initialItems(count: Int, weight: (Int) -> Int, screenWeight: Float): Int {
    var shown = 0
    var total = 0L
    while (shown < minOf(count, MAX_INITIAL_ITEMS) && (shown == 0 || total < screenWeight)) {
        total += weight(shown).coerceAtLeast(1)
        shown++
    }
    return shown
}

/**
 * Cap of the blank space standing for the items not on screen yet: a document of several megabytes must not claim a height
 * that Compose cannot carry around (a fixed height above 2^18 px does not fit in `Constraints` at a phone's width).
 */
private const val MAX_RESERVED_PX = 200_000L

/** A text up to this length is cut into pieces of lines at once; a longer one off the main thread ([rememberOffMain]). */
private const val SYNC_SOURCE_CHARS = 64_000

/**
 * [compute] of [text], remembered: at once for a text of [syncLimit] characters at most, else on a background thread, null
 * until it is done (the caller shows [PendingText] meanwhile). The first call of this code in a process is slow (classes,
 * regular expressions): on a long text it would freeze the transition that opens the screen. [compute] must be pure.
 */
@Composable
internal fun <T : Any> rememberOffMain(text: String, syncLimit: Int, compute: (String) -> T): T? {
    val state = remember(text) { mutableStateOf(if (text.length <= syncLimit) compute(text) else null) }
    if (state.value == null) {
        LaunchedEffect(text) { state.value = withContext(Dispatchers.Default) { compute(text) } }
    }
    return state.value
}

/**
 * Stands for a long text that is being prepared ([rememberOffMain]): a screen of blank space, so that what follows the text
 * does not show at the top and jump down a frame or two later. No progress indicator (charter).
 */
@Composable
internal fun PendingText(modifier: Modifier = Modifier) {
    val windowHeight = LocalWindowInfo.current.containerSize.height
    Spacer(modifier.fillMaxWidth().height(with(LocalDensity.current) { windowHeight.toDp() }))
}

/** The pieces of a text shown literally: their [start, end) ranges in the text and their weights. No content. */
private class SourcePieces(val ranges: List<IntArray>, val weights: IntArray)

/**
 * A long text shown literally (plain text, Markdown source, revealed secret) in the detail: pieces of whole lines
 * ([sourceChunkRanges]), each its own [Text], revealed progressively. A short text stays one [Text]. The pieces only live in
 * memory, and only those revealed are ever copied out of the text.
 */
@Composable
fun ChunkedText(text: String, style: TextStyle, modifier: Modifier = Modifier) {
    val pieces = rememberOffMain(text, SYNC_SOURCE_CHARS) { source ->
        val ranges = sourceChunkRanges(source, SOURCE_CHUNK_LINES, SOURCE_CHUNK_CHARS)
        SourcePieces(ranges, IntArray(ranges.size) { textWeight(source, ranges[it][0], ranges[it][1]) })
    }
    if (pieces == null) {
        PendingText(modifier)
        return
    }
    if (pieces.ranges.size <= 1) {
        Text(text, style = style, modifier = modifier)
        return
    }
    ProgressiveColumn(pieces.ranges.size, weight = { pieces.weights[it] }, resetKey = text, modifier = modifier) { index ->
        val piece = remember(text, index) { text.substring(pieces.ranges[index][0], pieces.ranges[index][1]) }
        Text(piece, style = style)
    }
}
