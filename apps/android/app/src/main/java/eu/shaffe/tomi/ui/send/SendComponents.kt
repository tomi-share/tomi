package eu.shaffe.tomi.ui.send

import android.os.SystemClock
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.send.UndoDeadline
import eu.shaffe.tomi.share.ExpiryChoice
import eu.shaffe.tomi.share.ExpiryPolicy
import eu.shaffe.tomi.ui.common.FeedbackRowPadding
import eu.shaffe.tomi.ui.common.TomiFeedbackSurface
import eu.shaffe.tomi.ui.common.expiryLabel
import eu.shaffe.tomi.ui.theme.TomiTheme
import kotlinx.coroutines.delay

private const val PROGRESS_STEP_MS = 50L

/** Expiry choices for the current secret state, "Never" (the default) first. "After first read" is for secrets without attachment only. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ExpiryChips(secret: Boolean, hasAttachment: Boolean, selected: ExpiryChoice, onSelect: (ExpiryChoice) -> Unit, modifier: Modifier = Modifier) {
    FlowRow(modifier = modifier, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        ExpiryPolicy.optionsFor(secret, hasAttachment).forEach { choice ->
            FilterChip(
                selected = choice == selected,
                onClick = { onSelect(choice) },
                label = { Text(expiryLabel(choice)) },
                leadingIcon = if (choice == selected) {
                    { Icon(Icons.Outlined.Check, contentDescription = null, modifier = Modifier.size(18.dp)) }
                } else {
                    null
                },
                shape = CircleShape,
            )
        }
    }
}

@Composable
fun expiryChipText(choice: ExpiryChoice): String = stringResource(R.string.expiry_chip, expiryLabel(choice))

/**
 * Bottom sheet used for the secret confirmation and for "Options" after a send: secret switch and expiry chips.
 * [preview] is already masked by the caller when the content is a secret.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SendOptionsSheet(
    preview: String,
    secret: Boolean,
    hasAttachment: Boolean,
    secretLocked: Boolean,
    expiry: ExpiryChoice,
    infoRes: Int?,
    confirmLabelRes: Int,
    cancelLabelRes: Int,
    onSecretChange: (Boolean) -> Unit,
    onExpiryChange: (ExpiryChoice) -> Unit,
    onConfirm: () -> Unit,
    onCancel: () -> Unit,
) {
    ModalBottomSheet(
        onDismissRequest = onCancel,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = TomiTheme.shapes.sheet,
        containerColor = MaterialTheme.colorScheme.surface,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp)
                .padding(bottom = 16.dp)
                .navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(stringResource(R.string.sheet_title), style = MaterialTheme.typography.titleLarge)
            Text(preview, style = MaterialTheme.typography.bodyMedium, maxLines = 3)
            if (infoRes != null) {
                Text(stringResource(infoRes), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary)
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.label_secret), style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                Switch(checked = secret, onCheckedChange = onSecretChange, enabled = !secretLocked)
            }
            Text(expiryChipText(expiry), style = MaterialTheme.typography.titleMedium)
            ExpiryChips(secret = secret, hasAttachment = hasAttachment, selected = expiry, onSelect = onExpiryChange)
            Button(onClick = onConfirm, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) { Text(stringResource(confirmLabelRes)) }
            TextButton(onClick = onCancel, modifier = Modifier.fillMaxWidth()) { Text(stringResource(cancelLabelRes)) }
        }
    }
}

/**
 * "Sent · Undo · Options" banner. The bar and the timeout follow [deadline], the real-time deadline shared with the
 * background job (not an animation: the animation scale of the developer options must not stretch the undo window).
 * [onTimeout] is called when the deadline is reached.
 */
@Composable
fun SentBanner(
    skippedFiles: Int,
    deadline: UndoDeadline,
    onUndo: () -> Unit,
    onOptions: () -> Unit,
    onTimeout: () -> Unit,
    modifier: Modifier = Modifier,
    elapsedRealtimeMs: () -> Long = { SystemClock.elapsedRealtime() },
) {
    var progress by remember(deadline) { mutableFloatStateOf(deadline.fraction(elapsedRealtimeMs())) }
    val currentOnTimeout by rememberUpdatedState(onTimeout)
    LaunchedEffect(deadline) {
        while (true) {
            val now = elapsedRealtimeMs()
            progress = deadline.fraction(now)
            if (deadline.isOver(now)) break
            delay(minOf(deadline.remainingMs(now), PROGRESS_STEP_MS))
        }
        currentOnTimeout()
    }
    TomiFeedbackSurface(modifier) {
        Column {
            LinearProgressIndicator(
                progress = { progress },
                modifier = Modifier.fillMaxWidth(),
                color = MaterialTheme.colorScheme.inversePrimary,
                trackColor = MaterialTheme.colorScheme.inverseSurface,
            )
            Row(Modifier.padding(FeedbackRowPadding), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.banner_sent), style = MaterialTheme.typography.bodyLarge)
                    if (skippedFiles > 0) {
                        Text(
                            pluralStringResource(R.plurals.banner_files_skipped, skippedFiles, skippedFiles),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
                TextButton(onClick = onUndo) { Text(stringResource(R.string.action_undo), color = MaterialTheme.colorScheme.inversePrimary) }
                TextButton(onClick = onOptions) { Text(stringResource(R.string.action_options), color = MaterialTheme.colorScheme.inversePrimary) }
            }
        }
    }
}
