package eu.shaffe.tomi.ui.pairing

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.PhoneAndroid
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DevicePlatform
import eu.shaffe.tomi.core.PairingPeer
import eu.shaffe.tomi.core.SensitiveText
import eu.shaffe.tomi.ui.common.CenteredScrollColumn
import eu.shaffe.tomi.ui.common.FlagSecureEffect
import eu.shaffe.tomi.ui.common.rememberAuthenticator
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.theme.TomiFonts
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * "Add a device": the code and the QR code of the invitation, then the comparison of the verification code with the new device
 * (spec/pairing-v1.md 7.2). A secure window: the code, the QR code and the verification code never reach a screenshot, a recording
 * or the recents. The code is never copied or shared from here.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InviteScreen(viewModel: InviteViewModel, onBack: () -> Unit) {
    FlagSecureEffect(enabled = true)
    // The fresh check that opens "Add a device" and the final confirmation need the current activity (BiometricPrompt).
    val authenticator = rememberAuthenticator()
    DisposableEffect(authenticator) {
        viewModel.authenticator = authenticator
        onDispose { if (viewModel.authenticator === authenticator) viewModel.authenticator = null }
    }
    LaunchedEffect(Unit) { viewModel.start() }
    val state = viewModel.state

    fun leave() {
        viewModel.abort()
        onBack()
    }
    // Back from the comparison is a refusal (a new code is made); from anywhere else it ends the invitation.
    BackHandler {
        when (state) {
            is InviteUiState.Verify -> viewModel.refuse()
            else -> leave()
        }
    }
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.invite_title)) },
                navigationIcon = {
                    IconButton(onClick = ::leave) {
                        Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_back))
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
    ) { padding ->
        Box(Modifier.screenContentPadding(padding).fillMaxSize()) {
            when (state) {
                InviteUiState.Starting -> StartingContent(onCancel = ::leave)
                is InviteUiState.ShowCode -> ShowCodeContent(state, onCancel = ::leave)
                is InviteUiState.Verify -> InviteVerifyContent(state, onAdd = viewModel::add, onRefuse = viewModel::refuse)
                is InviteUiState.Adding -> AddingContent(state.peer, onCancel = ::leave)
                is InviteUiState.AuthRequired -> AuthRequiredContent(state, onRetry = viewModel::restart, onClose = ::leave)
                is InviteUiState.Done -> DoneContent(state, onFinished = onBack)
                is InviteUiState.Failed -> FailedContent(state, onRetry = viewModel::restart, onClose = onBack)
            }
        }
    }
}

@Composable
private fun StartingContent(onCancel: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.invite_starting), style = MaterialTheme.typography.bodyLarge)
            // Charter 6.3: progress is linear, never a spinner.
            LinearProgressIndicator(Modifier.fillMaxWidth())
        }
        BottomActions {
            TextButton(onClick = onCancel, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.action_cancel))
            }
        }
    }
}

/** The code and the QR code: the same secret, two ways to enter it. */
@Composable
private fun ShowCodeContent(state: InviteUiState.ShowCode, onCancel: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            state.notice?.let {
                Text(stringResource(it), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            }
            Text(
                stringResource(R.string.invite_intro),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            QrCodeView(
                content = state.link.reveal(),
                description = stringResource(R.string.invite_qr_description),
                modifier = Modifier.widthIn(max = 280.dp).fillMaxWidth(),
            )
            PairingCodeText(state.code)
            ValidityCountdown(state.deadlineElapsedMs, Modifier.fillMaxWidth())
            Text(
                stringResource(R.string.invite_single_use),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
        }
        BottomActions {
            TextButton(onClick = onCancel, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.action_cancel))
            }
        }
    }
}

/** `4821-crayon-renard`, large, monospace. Shown, never copied: there is no copy action and no selection on purpose. */
@Composable
private fun PairingCodeText(code: SensitiveText) {
    val words = code.reveal()
    Text(
        words,
        style = MaterialTheme.typography.titleLarge.copy(
            fontFamily = TomiFonts.mono,
            fontSize = 24.sp,
            fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES,
            fontWeight = FontWeight.SemiBold,
        ),
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().semantics { contentDescription = words.replace('-', ' ') },
    )
}

/** The verification code of the new device: compare it with the one on its screen, then add it. */
@Composable
private fun InviteVerifyContent(state: InviteUiState.Verify, onAdd: () -> Unit, onRefuse: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.pair_verify_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.invite_verify_body, state.peer.name),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(16.dp))
            PeerRow(state.peer)
            Spacer(Modifier.height(24.dp))
            VerificationCodeBoxes(state.verificationCode)
            if (state.authDenied) {
                Text(
                    stringResource(R.string.invite_notice_auth_denied),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                )
            }
            // If the other device shows an error instead of a code (the code was already used), the answer is "no" (spec 5.3).
            Text(
                stringResource(R.string.invite_verify_hint),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
            )
        }
        BottomActions {
            Button(onClick = onAdd, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.invite_add))
            }
            TextButton(onClick = onRefuse, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.pair_verify_different))
            }
        }
    }
}

@Composable
private fun PeerRow(peer: PairingPeer) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Surface(shape = CircleShape, color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.size(40.dp)) {
            Box(contentAlignment = Alignment.Center) {
                Icon(if (peer.platform == DevicePlatform.Android) Icons.Outlined.PhoneAndroid else Icons.Outlined.Computer, contentDescription = null)
            }
        }
        Column {
            Text(peer.name, style = MaterialTheme.typography.titleMedium)
            Text(
                stringResource(
                    when (peer.platform) {
                        DevicePlatform.Android -> R.string.platform_android
                        DevicePlatform.Windows -> R.string.platform_windows
                        DevicePlatform.Other -> R.string.platform_other
                    },
                ),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun AddingContent(peer: PairingPeer, onCancel: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.invite_adding_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.invite_adding_body, peer.name),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(8.dp))
            LinearProgressIndicator(Modifier.fillMaxWidth())
        }
        BottomActions {
            TextButton(onClick = onCancel, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.action_cancel))
            }
        }
    }
}

@Composable
private fun DoneContent(state: InviteUiState.Done, onFinished: () -> Unit) {
    Column(Modifier.fillMaxSize().padding(24.dp)) {
        Column(
            Modifier.weight(1f).fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(Icons.Outlined.Check, contentDescription = null, modifier = Modifier.size(72.dp), tint = TomiTheme.colors.success)
            Text(stringResource(R.string.pair_done_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.pair_done_body, state.peer.name),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            if (state.historyAnnounced) {
                Text(
                    stringResource(R.string.invite_done_history, state.peer.name),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = TextAlign.Center,
                )
            }
        }
        BottomActions {
            Button(onClick = onFinished, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.action_done))
            }
        }
    }
}

@Composable
private fun FailedContent(state: InviteUiState.Failed, onRetry: () -> Unit, onClose: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.invite_failed_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(inviteFailureMessage(state.reason)),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        BottomActions {
            Button(onClick = onRetry, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.invite_new_code))
            }
            TextButton(onClick = onClose, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.action_close))
            }
        }
    }
}

/** The fresh check of the user was refused or cancelled: nothing was opened at the relay. */
@Composable
private fun AuthRequiredContent(state: InviteUiState.AuthRequired, onRetry: () -> Unit, onClose: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.invite_auth_required_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(if (state.unavailable) R.string.invite_auth_unavailable_body else R.string.invite_auth_required_body),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        BottomActions {
            if (!state.unavailable) {
                Button(onClick = onRetry, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                    Text(stringResource(R.string.invite_auth_retry))
                }
            }
            TextButton(onClick = onClose, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                Text(stringResource(R.string.action_close))
            }
        }
    }
}
