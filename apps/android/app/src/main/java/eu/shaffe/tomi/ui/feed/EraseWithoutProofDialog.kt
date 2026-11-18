package eu.shaffe.tomi.ui.feed

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import eu.shaffe.tomi.R

/**
 * "Effacer quand même" (spec/api-v1.md 7.9.5, D4): the only erasure without a proof of the relay. Honest text (the space may still exist
 * on the server), a checkbox, a button that works 2 s after the dialog shows, and only while the core allows it; the strong biometric
 * prompt comes after the button. Nothing here decides: [LeaveSpaceViewModel] does, and the core has the last word.
 */
@Composable
fun EraseWithoutProofDialog(
    state: LeaveUiState.EraseWithoutProof,
    attemptRunning: Boolean,
    onChecked: (Boolean) -> Unit,
    onConfirm: () -> Unit,
    onRecheck: () -> Unit,
    onDismiss: () -> Unit,
) {
    // While it waits for an attempt of the departure to end, nothing has been asked of the core: the dialog can be left.
    val locked = (state.busy || state.authenticating) && !state.waiting
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(dismissOnBackPress = !locked, dismissOnClickOutside = !locked),
        title = { Text(stringResource(R.string.erase_title)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                DialogBody(
                    messages = listOf(stringResource(R.string.erase_message), stringResource(R.string.erase_detail)),
                    busyText = stringResource(
                        when {
                            state.waiting -> R.string.erase_waiting
                            state.busy -> R.string.erase_in_progress
                            else -> R.string.erase_checking
                        },
                    ),
                    busy = state.busy || state.checking,
                    error = when {
                        state.biometricUnavailable -> stringResource(R.string.erase_unavailable_biometric)
                        state.expired -> stringResource(R.string.erase_expired)
                        state.notAllowed -> stringResource(R.string.erase_not_allowed)
                        state.failed -> stringResource(R.string.departure_failed)
                        else -> null
                    },
                    modifier = Modifier,
                )
                Row(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 48.dp)
                        .toggleable(value = state.checked, enabled = !locked, role = Role.Checkbox, onValueChange = onChecked),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    // The row carries the toggle (and its semantics); the box only shows it.
                    Checkbox(checked = state.checked, onCheckedChange = null, enabled = !locked)
                    Text(stringResource(R.string.erase_checkbox), style = MaterialTheme.typography.bodyMedium)
                }
            }
        },
        confirmButton = {
            if (state.notAllowed && !state.checking) {
                DialogTextButton(stringResource(R.string.departure_retry), onRecheck)
            } else {
                DangerButton(stringResource(R.string.erase_confirm), enabled = state.canConfirm && !attemptRunning, onClick = onConfirm)
            }
        },
        dismissButton = { DialogTextButton(stringResource(R.string.action_cancel), onDismiss, enabled = !locked) },
    )
}
