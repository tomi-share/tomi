package eu.shaffe.tomi.ui.feed

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DepartureKind
import eu.shaffe.tomi.core.PendingReason
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * The state of the departure, at the top of the feed, for as long as there is one (spec/api-v1.md 7.9.2: the interface shows the state permanently),
 * across restarts: it reads the marker of the core. Not dismissable. "Réessayer" is off while an attempt runs; "Annuler"
 * never is, except during the erasure itself. No icon on the buttons (no circular arrows, charter 6.3); progress is a linear bar.
 */
@Composable
fun DepartureBanner(
    state: DepartureBannerState,
    onRetry: () -> Unit,
    onCancel: () -> Unit,
    onEraseAnyway: () -> Unit,
    onLeave: () -> Unit,
    modifier: Modifier = Modifier,
) {
    when (state) {
        DepartureBannerState.None -> Unit
        is DepartureBannerState.Pending -> BannerCard(
            title = stringResource(
                if (state.kind == DepartureKind.Leave) R.string.departure_pending_leave_title else R.string.departure_pending_delete_title,
            ),
            text = stringResource(pendingText(state.last)),
            progress = state.running,
            modifier = modifier,
        ) {
            BannerButton(stringResource(R.string.departure_retry), enabled = !state.running && !state.authenticating, onClick = onRetry)
            BannerButton(stringResource(R.string.departure_cancel), enabled = true, onClick = onCancel)
            if (state.offersEraseAnyway) BannerButton(stringResource(R.string.erase_anyway), enabled = true, danger = true, onClick = onEraseAnyway)
        }
        DepartureBannerState.Wiping -> BannerCard(
            title = null,
            text = stringResource(R.string.departure_wiping),
            progress = true,
            modifier = modifier,
        )
        DepartureBannerState.Cancelling -> BannerCard(
            title = stringResource(R.string.departure_cancelling_title),
            text = stringResource(R.string.departure_cancelling),
            progress = true,
            modifier = modifier,
        )
        DepartureBannerState.SpaceNotFound -> BannerCard(
            title = stringResource(R.string.space_not_found_title),
            text = stringResource(R.string.space_not_found_message),
            progress = false,
            modifier = modifier,
        ) {
            BannerButton(stringResource(R.string.erase_anyway), enabled = true, danger = true, onClick = onEraseAnyway)
        }
        DepartureBannerState.Revoked -> BannerCard(
            title = stringResource(R.string.space_revoked_title),
            text = stringResource(R.string.space_revoked_message),
            progress = false,
            modifier = modifier,
        ) {
            BannerButton(stringResource(R.string.leave_confirm), enabled = true, danger = true, onClick = onLeave)
        }
    }
}

private fun pendingText(last: PendingReason?): Int = when (last) {
    null -> R.string.departure_pending_checking
    PendingReason.Network -> R.string.departure_pending_network
    PendingReason.Server -> R.string.departure_pending_server
    PendingReason.NotSent -> R.string.departure_pending_not_sent
    PendingReason.NotFound -> R.string.departure_pending_not_found
}

@Composable
private fun BannerCard(
    title: String?,
    text: String,
    progress: Boolean,
    modifier: Modifier = Modifier,
    buttons: (@Composable () -> Unit)? = null,
) {
    Surface(
        modifier = modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
        shape = TomiTheme.shapes.card,
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        contentColor = MaterialTheme.colorScheme.onSurface,
    ) {
        Column(
            Modifier.padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = if (buttons == null) 12.dp else 4.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            // Announced when it changes: the state of a departure that runs in the background.
            Column(Modifier.padding(end = 8.dp).semantics { liveRegion = LiveRegionMode.Polite }, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (title != null) Text(title, style = MaterialTheme.typography.titleSmall)
                Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (progress) LinearProgressIndicator(Modifier.fillMaxWidth().padding(end = 8.dp))
            if (buttons != null) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) { buttons() }
            }
        }
    }
}

@Composable
private fun BannerButton(text: String, enabled: Boolean, onClick: () -> Unit, danger: Boolean = false) {
    TextButton(
        onClick = onClick,
        enabled = enabled,
        colors = if (danger) ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error) else ButtonDefaults.textButtonColors(),
        modifier = Modifier.heightIn(min = 48.dp),
    ) { Text(text) }
}
