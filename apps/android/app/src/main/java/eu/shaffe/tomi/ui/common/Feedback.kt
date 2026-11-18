package eu.shaffe.tomi.ui.common

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.SnackbarVisuals
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.ui.theme.TomiTheme
import kotlinx.coroutines.delay

/*
 * The one feedback of the app.
 *
 * The confirmation of a send is the "Sent / Undo / Options" banner (`SentBanner`, ui/send), shown for the undo window of
 * `SendCoordinator.UNDO_WINDOW_MS`. Every other one-line confirmation (copied, deleted, image saved, ...) is drawn by the
 * same surface ([TomiFeedbackSurface]), with the same text style, the same paddings, its own duration ([FEEDBACK_DURATION_MS])
 * and the same position: 16 dp from the screen edges, 96 dp above the floating toolbar (the host sits where the banner sits).
 * Only the undo progress bar is specific to the banner: it shows the time left to undo, which a copy does not have.
 *
 * Style (2026-11-11): a DARK ink card in both modes, the floating layer of the app.
 * Light mode: ink `color.light.text` #1B2130, the old inverse surface. Dark mode: `color.dark.surface-alt` #242A38 with a 1 dp
 * `color.dark.border` hairline (no more cream slab on a dark screen, which the inverse surface of Material gave). Accents are
 * the dark-mode tokens in both modes (they are the ones made for a dark background): actions in brand #A9BFEA, the check of a
 * done action in success #8FCFA6. Corners of a floating layer (`TomiTheme.shapes.menu`: radius.lg 12 dp, 20 dp in the
 * expressive variant), at most [FeedbackMaxWidth] wide. The identity colours (night #24324A, sun #FFC85C) are not used.
 * A confirmation of an action that worked ([FeedbackTone.Done]: copied, saved, deleted, archived) starts with a discreet
 * check; the others (failures, "unavailable") have no icon.
 *
 * Inside the surface, the inverse roles of Material ([androidx.compose.material3.ColorScheme.inverseSurface],
 * `inverseOnSurface`, `inversePrimary`) are the ones of the feedback: the banner draws its bar and its buttons with them.
 *
 * An ordinary copy has NO feedback of the app. Android 13+ (minSdk is 33) shows its own system confirmation when an app writes
 * to the clipboard; no public API turns it off (`ClipDescription.EXTRA_IS_SENSITIVE` only hides the preview of the content; the
 * overlay's "suppress" extra is honoured by SystemUI on emulators only), so a second one from the app would make two. Only the
 * copy of a hidden item has one ("Copied · cleared in 30 s"): it says what the system does not.
 */

/** How long a confirmation stays on screen (long enough to be read; independent of the 3 s undo window of the send banner). */
const val FEEDBACK_DURATION_MS: Long = 5_000L

/** Inner padding of the row of a feedback (banner and snackbar): same values for both. */
val FeedbackRowPadding = PaddingValues(start = 16.dp, end = 8.dp, top = 4.dp, bottom = 4.dp)

/** Height of the content row of a feedback, the one of a row holding a text button. */
private val FeedbackRowMinHeight = 48.dp

/** A feedback does not stretch across a tablet or a landscape screen. */
private val FeedbackMaxWidth = 560.dp

/** What a confirmation says: [Done] for an action that worked (a check in front), [Neutral] for everything else. */
enum class FeedbackTone { Neutral, Done }

/**
 * Colours of a feedback, from `design/tomi/tokens.json`. Plain ARGB ints so that it is unit tested without Android
 * (contrast, no identity colour).
 */
class FeedbackPalette(
    val container: Int,
    val content: Int,
    /** Text buttons ("Undo", "Options") and the undo bar of the banner. */
    val action: Int,
    /** Check of a [FeedbackTone.Done] confirmation. */
    val done: Int,
    /** Hairline around the card, where the card would not stand out of the screen without it; null: none. */
    val border: Int?,
) {
    companion object {
        val Light = FeedbackPalette(
            container = 0xFF1B2130.toInt(),
            content = 0xFFF6F4EF.toInt(),
            action = 0xFFA9BFEA.toInt(),
            done = 0xFF8FCFA6.toInt(),
            border = null,
        )

        val Dark = FeedbackPalette(
            container = 0xFF242A38.toInt(),
            content = 0xFFE9E7E1.toInt(),
            action = 0xFFA9BFEA.toInt(),
            done = 0xFF8FCFA6.toInt(),
            border = 0xFF383E4C.toInt(),
        )

        fun of(dark: Boolean): FeedbackPalette = if (dark) Dark else Light
    }
}

/** The palette of the current theme (dark when the surface of the scheme is dark, whatever the system setting). */
@Composable
private fun feedbackPalette(): FeedbackPalette = FeedbackPalette.of(MaterialTheme.colorScheme.surface.luminance() < 0.5f)

/** The card of a feedback. [modifier] carries the margins (the card fills the width left, up to [FeedbackMaxWidth]). */
@Composable
fun TomiFeedbackSurface(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val palette = feedbackPalette()
    val container = Color(palette.container)
    val onContainer = Color(palette.content)
    Surface(
        modifier = modifier.widthIn(max = FeedbackMaxWidth).fillMaxWidth(),
        shape = TomiTheme.shapes.menu,
        color = container,
        contentColor = onContainer,
        shadowElevation = 6.dp,
        border = palette.border?.let { BorderStroke(1.dp, Color(it)) },
    ) {
        MaterialTheme(
            colorScheme = MaterialTheme.colorScheme.copy(
                inverseSurface = container,
                inverseOnSurface = onContainer,
                inversePrimary = Color(palette.action),
            ),
            typography = MaterialTheme.typography,
            shapes = MaterialTheme.shapes,
            content = content,
        )
    }
}

/** Visuals of a feedback: Material's, plus its [tone]. */
private class TomiFeedbackVisuals(
    override val message: String,
    override val actionLabel: String?,
    val tone: FeedbackTone,
) : SnackbarVisuals {
    override val withDismissAction: Boolean get() = false
    override val duration: SnackbarDuration get() = SnackbarDuration.Indefinite
}

/**
 * Host of the one-line confirmations, to be used instead of Material's `SnackbarHost`: draws every message with
 * [TomiFeedbackSurface] (the card of the send banner) and dismisses it after [FEEDBACK_DURATION_MS]. Show a message with
 * [showFeedback].
 */
@Composable
fun TomiSnackbarHost(state: SnackbarHostState, modifier: Modifier = Modifier) {
    SnackbarHost(state, modifier = modifier) { data ->
        LaunchedEffect(data) {
            delay(FEEDBACK_DURATION_MS)
            data.dismiss()
        }
        val tone = (data.visuals as? TomiFeedbackVisuals)?.tone ?: FeedbackTone.Neutral
        val palette = feedbackPalette()
        TomiFeedbackSurface(Modifier.padding(horizontal = 16.dp)) {
            Row(
                Modifier.heightIn(min = FeedbackRowMinHeight).padding(FeedbackRowPadding),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (tone == FeedbackTone.Done) {
                    // Decorative: the message says what happened.
                    Icon(Icons.Outlined.Check, contentDescription = null, tint = Color(palette.done), modifier = Modifier.size(20.dp))
                    Spacer(Modifier.width(12.dp))
                }
                Text(
                    data.visuals.message,
                    style = MaterialTheme.typography.bodyLarge,
                    modifier = Modifier.weight(1f).padding(vertical = 8.dp),
                )
                data.visuals.actionLabel?.let { label ->
                    TextButton(onClick = data::performAction) {
                        Text(label, color = MaterialTheme.colorScheme.inversePrimary)
                    }
                }
            }
        }
    }
}

/**
 * Shows [message] in a [TomiSnackbarHost]. A message still on screen is replaced at once (a second copy does not wait
 * for the first one to time out). The duration is set by the host, not by the snackbar state. [tone] adds the check of a
 * done action.
 */
suspend fun SnackbarHostState.showFeedback(
    message: String,
    actionLabel: String? = null,
    tone: FeedbackTone = FeedbackTone.Neutral,
): SnackbarResult {
    currentSnackbarData?.dismiss()
    return showSnackbar(TomiFeedbackVisuals(message, actionLabel, tone))
}
