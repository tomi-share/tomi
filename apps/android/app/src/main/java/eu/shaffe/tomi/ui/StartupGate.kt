package eu.shaffe.tomi.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.CoreStartup
import eu.shaffe.tomi.R
import eu.shaffe.tomi.StartupState
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.theme.TomiLogo

/**
 * Shows [content] once the core is open. While it opens (a fraction of a second, off the main thread) only the background is drawn: no
 * spinner, no animation near the symbol (charter 6.3). When it cannot be opened, [UnreadableDataScreen] offers a way out.
 */
@Composable
fun StartupGate(startup: CoreStartup<AppContainer>, content: @Composable () -> Unit) {
    val state by startup.state.collectAsStateWithLifecycle()
    when (val current = state) {
        StartupState.Opening -> Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background))
        is StartupState.Ready -> content()
        is StartupState.Unreadable -> UnreadableDataScreen(
            resetFailed = current.resetFailed,
            onRetry = startup::retry,
            onReset = startup::reset,
        )
    }
}

/**
 * "Données illisibles": the Keystore key that protects the database is gone, or the database cannot be read. "Réessayer" opens again
 * (a Keystore that was not ready); "Réinitialiser" erases the local data, behind a confirmation that says what it costs.
 */
@Composable
fun UnreadableDataScreen(resetFailed: Boolean, onRetry: () -> Unit, onReset: () -> Unit) {
    var confirming by remember { mutableStateOf(false) }
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
    ) { padding ->
        Column(
            Modifier.screenContentPadding(padding).fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            TomiLogo(height = 32.dp, tint = MaterialTheme.colorScheme.onBackground)
            Text(
                stringResource(R.string.unreadable_title),
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.padding(top = 8.dp),
            )
            Text(
                stringResource(R.string.unreadable_message),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (resetFailed) {
                Text(
                    stringResource(R.string.unreadable_reset_failed),
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.error,
                )
            }
            Button(onClick = onRetry, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.unreadable_retry)) }
            OutlinedButton(onClick = { confirming = true }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.unreadable_reset))
            }
        }
    }

    if (confirming) {
        AlertDialog(
            onDismissRequest = { confirming = false },
            title = { Text(stringResource(R.string.unreadable_reset_title)) },
            text = { Text(stringResource(R.string.unreadable_reset_message)) },
            confirmButton = {
                TextButton(onClick = {
                    confirming = false
                    onReset()
                }) { Text(stringResource(R.string.unreadable_reset_confirm), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirming = false }) { Text(stringResource(R.string.action_cancel)) } },
        )
    }
}
