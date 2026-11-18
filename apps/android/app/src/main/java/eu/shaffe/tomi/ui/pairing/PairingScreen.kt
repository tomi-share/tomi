package eu.shaffe.tomi.ui.pairing

import android.Manifest
import android.content.pm.PackageManager
import android.text.format.Formatter
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.QrCode2
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DevicePlatform
import eu.shaffe.tomi.core.RelayAddress
import eu.shaffe.tomi.core.VerificationCode
import eu.shaffe.tomi.ui.common.CenteredScrollColumn
import eu.shaffe.tomi.ui.common.FlagSecureEffect
import eu.shaffe.tomi.ui.common.appContainer
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.theme.TomiFonts
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * "Join a space" (the new device): QR scan or typed invitation code, then the check of the 6-digit code, then ONE yes/no question about
 * the history, then the wait for the person on the other device. The camera permission is requested only when the user taps a scan button,
 * and the notification permission only once pairing succeeded.
 *
 * The first step has two screens: the scan ("Rejoins ton espace") and, behind "Saisir le code d'invitation", the typing of the code
 * ([enteringCode], local to this screen). [onBack] leaves the first step towards the welcome screen: nothing is going on at that point.
 *
 * The whole screen is a secure window (no screenshot, no recording, blank in the recents): the code that is typed or scanned and the
 * verification code are secrets of the pairing (spec/pairing-v1.md 12).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PairingScreen(
    viewModel: PairingViewModel,
    onFinished: () -> Unit,
    onBack: () -> Unit,
) {
    FlagSecureEffect(enabled = true)
    var enteringCode by rememberSaveable { mutableStateOf(false) }
    // Leaving any step of the pairing counts as a refusal: nothing is joined, and the channel is closed at the relay.
    BackHandler(enabled = viewModel.phase != PairingPhase.Input && viewModel.phase != PairingPhase.Done) {
        viewModel.leave()
    }
    // Back from the typing of the code returns to the scan, it does not leave the screen.
    BackHandler(enabled = viewModel.phase == PairingPhase.Input && enteringCode) { enteringCode = false }
    val navigateUp: () -> Unit = {
        when {
            viewModel.phase != PairingPhase.Input -> viewModel.leave()
            enteringCode -> enteringCode = false
            else -> onBack()
        }
    }
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        // Status bar, gesture bar, cutout and keyboard: see Insets.kt. The TopAppBar draws behind the status bar by itself.
        contentWindowInsets = tomiContentInsets,
        topBar = {
            if (viewModel.phase != PairingPhase.Done) {
                TopAppBar(
                    title = {},
                    navigationIcon = {
                        IconButton(onClick = navigateUp) {
                            Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_back))
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
                )
            }
        },
    ) { padding ->
        Box(Modifier.screenContentPadding(padding).fillMaxSize()) {
            when (viewModel.phase) {
                PairingPhase.Done -> JoinedContent(onFinished)
                PairingPhase.Verify -> VerifyContent(viewModel)
                PairingPhase.History -> HistoryContent(viewModel)
                PairingPhase.Waiting -> WaitingContent(viewModel)
                else -> PairingInputContent(viewModel, enteringCode, onEnteringCodeChange = { enteringCode = it })
            }
        }
    }
}

/** An example of what the other device shows, for the illustration only. It is not a code. */
private const val EXAMPLE_INVITATION_CODE = "7-crayon-renard"

/**
 * First step, scan or typed code. The camera permission and the scanner live here so that both screens ("Rejoins ton espace" and
 * "Saisis le code d'invitation") can start a scan; the permission is only asked when a scan button is tapped.
 */
@Composable
private fun PairingInputContent(viewModel: PairingViewModel, enteringCode: Boolean, onEnteringCodeChange: (Boolean) -> Unit) {
    val context = LocalContext.current
    val working = viewModel.phase == PairingPhase.Working
    var scanning by remember { mutableStateOf(false) }
    var cameraDenied by remember { mutableStateOf(false) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        cameraDenied = !granted
        scanning = granted
    }
    val startScan: () -> Unit = {
        val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        if (granted) scanning = true else launcher.launch(Manifest.permission.CAMERA)
    }
    // Back closes the scanner before it leaves the screen.
    BackHandler(enabled = scanning) { scanning = false }

    if (enteringCode) {
        CodeEntryContent(
            viewModel = viewModel,
            working = working,
            onScanInstead = {
                onEnteringCodeChange(false)
                startScan()
            },
        )
    } else {
        ScanContent(
            viewModel = viewModel,
            working = working,
            scanning = scanning,
            cameraDenied = cameraDenied,
            onScanClick = startScan,
            onScanned = {
                scanning = false
                viewModel.onQrScanned(it)
            },
            onEnterCode = {
                scanning = false
                onEnteringCodeChange(true)
            },
        )
    }
}

/** "Rejoins ton espace": what to look for on the other device, then the scan (or the way to the typed code). */
@Composable
private fun ScanContent(
    viewModel: PairingViewModel,
    working: Boolean,
    scanning: Boolean,
    cameraDenied: Boolean,
    onScanClick: () -> Unit,
    onScanned: (String) -> Unit,
    onEnterCode: () -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(stringResource(R.string.pair_title), style = MaterialTheme.typography.titleLarge)
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(
                    stringResource(R.string.pair_intro),
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                OtherDevicePath()
            }
            PendingLinkCard(viewModel, working)
            if (scanning && !working) {
                val previewDescription = stringResource(R.string.pair_scan_preview_description)
                QrScanner(
                    onScanned = onScanned,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(320.dp)
                        .clip(TomiTheme.shapes.menu)
                        .semantics { contentDescription = previewDescription },
                )
            } else {
                OtherDeviceIllustration()
            }
            Text(
                stringResource(R.string.pair_scan_hint),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
            if (cameraDenied) {
                Text(stringResource(R.string.pair_camera_denied), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
            }
            PairingStatus(viewModel, working)
        }
        BottomActions {
            Button(
                onClick = onScanClick,
                enabled = !working,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) {
                Icon(Icons.Outlined.QrCodeScanner, contentDescription = null, modifier = Modifier.size(20.dp))
                Text(stringResource(R.string.pair_scan_button), modifier = Modifier.padding(start = 8.dp))
            }
            Spacer(Modifier.height(4.dp))
            OutlinedButton(
                onClick = onEnterCode,
                enabled = !working,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) {
                Icon(Icons.Outlined.Keyboard, contentDescription = null, modifier = Modifier.size(20.dp))
                Text(stringResource(R.string.pair_enter_code_button), modifier = Modifier.padding(start = 8.dp))
            }
        }
    }
}

/** "Appareils › Ajouter un appareil" on one line that does not break: the chevron stands between two labels that stay whole. */
@Composable
private fun OtherDevicePath() {
    Row(Modifier.semantics(mergeDescendants = true) {}, verticalAlignment = Alignment.CenterVertically) {
        Text(
            stringResource(R.string.devices_title),
            style = MaterialTheme.typography.bodyLarge,
            fontWeight = FontWeight.Medium,
            softWrap = false,
        )
        Icon(
            Icons.AutoMirrored.Outlined.KeyboardArrowRight,
            contentDescription = null,
            modifier = Modifier.padding(horizontal = 2.dp).size(20.dp),
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            stringResource(R.string.devices_add),
            style = MaterialTheme.typography.bodyLarge,
            fontWeight = FontWeight.Medium,
            softWrap = false,
        )
    }
}

/** What the "Add a device" screen of the other device looks like: a title, a QR pictogram (not a real code) and an example of the code. */
@Composable
private fun OtherDeviceIllustration() {
    val description = stringResource(R.string.pair_illustration_description)
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            stringResource(R.string.pair_other_device_caption),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Surface(
            shape = TomiTheme.shapes.card,
            color = MaterialTheme.colorScheme.surface,
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            modifier = Modifier.clearAndSetSemantics { contentDescription = description },
        ) {
            Column(
                Modifier.padding(horizontal = 24.dp, vertical = 16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(stringResource(R.string.devices_add), style = MaterialTheme.typography.labelLarge)
                Icon(
                    Icons.Outlined.QrCode2,
                    contentDescription = null,
                    modifier = Modifier.size(112.dp),
                    tint = MaterialTheme.colorScheme.onSurface,
                )
                Text(
                    EXAMPLE_INVITATION_CODE,
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = TomiFonts.mono, fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

/** "Saisis le code d'invitation": the typed code, with the same keyboard options as ever (no capital, no correction, ASCII). */
@Composable
private fun CodeEntryContent(viewModel: PairingViewModel, working: Boolean, onScanInstead: () -> Unit) {
    val focusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { focusRequester.requestFocus() }
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(stringResource(R.string.pair_code_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.pair_code_intro),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            PendingLinkCard(viewModel, working)
            OutlinedTextField(
                value = viewModel.code,
                onValueChange = viewModel::onCodeChange,
                label = { Text(stringResource(R.string.pair_code_label)) },
                supportingText = { Text(stringResource(R.string.pair_code_hint)) },
                singleLine = true,
                shape = TomiTheme.shapes.field,
                textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = TomiFonts.mono, fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
                // The code is two words of a fixed list: no correction, no capital, no suggestion that a keyboard would keep.
                keyboardOptions = KeyboardOptions(
                    capitalization = KeyboardCapitalization.None,
                    autoCorrectEnabled = false,
                    keyboardType = KeyboardType.Ascii,
                    imeAction = ImeAction.Done,
                ),
                modifier = Modifier.fillMaxWidth().focusRequester(focusRequester),
            )
            RelayField(
                value = viewModel.relayText,
                onValueChange = viewModel::onRelayChange,
                required = viewModel.relayRequired,
                configuredRelay = viewModel.configuredRelay,
                enabled = !working,
                imeAction = ImeAction.Done,
            )
            PairingStatus(viewModel, working)
        }
        BottomActions {
            Button(
                onClick = viewModel::joinWithCode,
                enabled = !working && viewModel.code.isNotBlank(),
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text(stringResource(R.string.pair_join)) }
            TextButton(
                onClick = onScanInstead,
                enabled = !working,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) {
                Icon(Icons.Outlined.QrCodeScanner, contentDescription = null, modifier = Modifier.size(20.dp))
                Text(stringResource(R.string.pair_scan_instead), modifier = Modifier.padding(start = 8.dp))
            }
        }
    }
}

/** A pairing link received from outside (App Link, QR code of the camera app): offered, never used without a tap. */
@Composable
private fun PendingLinkCard(viewModel: PairingViewModel, working: Boolean) {
    viewModel.pendingPayload?.let {
        Surface(
            shape = TomiTheme.shapes.card,
            color = MaterialTheme.colorScheme.surface,
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.pair_link_received), style = MaterialTheme.typography.titleMedium)
                viewModel.pendingRelay?.let {
                    Text(
                        stringResource(R.string.pair_link_received_relay, RelayAddress.authority(it)),
                        style = MaterialTheme.typography.bodyLarge.copy(fontFamily = TomiFonts.mono, fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
                    )
                }
                Text(stringResource(R.string.pair_link_received_hint), style = MaterialTheme.typography.bodyMedium)
                Button(onClick = viewModel::joinWithPendingPayload, enabled = !working, shape = TomiTheme.shapes.button) {
                    Text(stringResource(R.string.pair_join))
                }
            }
        }
    }
}

/** The error of the last attempt and, while the other device is being reached, the progress bar with the way out. */
@Composable
private fun PairingStatus(viewModel: PairingViewModel, working: Boolean) {
    viewModel.error?.let {
        Text(stringResource(it), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
    }
    if (working) {
        // Charter 6.3: progress is linear, never a spinner.
        LinearProgressIndicator(Modifier.fillMaxWidth())
        Text(
            stringResource(R.string.pair_working),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        // Joining waits for the other device, which can take a while: the way out is always there.
        TextButton(onClick = viewModel::leave, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.action_cancel)) }
    }
}

/** Step 2: the same 6-digit code must be shown on both devices. */
@Composable
private fun VerifyContent(viewModel: PairingViewModel) {
    val awaiting = viewModel.awaiting ?: return
    val members = awaiting.memberCount
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.pair_verify_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.pair_verify_body),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(16.dp))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Surface(shape = CircleShape, color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.size(40.dp)) {
                    Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.Computer, contentDescription = null) }
                }
                Column {
                    Text(awaiting.peer.name, style = MaterialTheme.typography.titleMedium)
                    Text(
                        if (members != null) {
                            pluralStringResource(R.plurals.pair_verify_peer_members, members, members)
                        } else {
                            stringResource(platformLabel(awaiting.peer.platform))
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            Spacer(Modifier.height(24.dp))
            VerificationCodeBoxes(awaiting.verificationCode)
            // The relay of this pairing may have been read from a QR code or typed: it is shown in full, next to the code, to be checked.
            if (awaiting.relayAuthority.isNotBlank()) {
                Spacer(Modifier.height(8.dp))
                RelayCheckCard(awaiting.relayAuthority, R.string.pair_verify_relay_label, R.string.pair_verify_relay_hint)
            }
            ValidityCountdown(viewModel.validUntilElapsedMs, Modifier.fillMaxWidth().padding(top = 8.dp))
        }
        BottomActions {
            Button(
                onClick = viewModel::codesMatch,
                enabled = !viewModel.busy,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text(stringResource(R.string.pair_verify_same)) }
            TextButton(
                onClick = viewModel::codesDiffer,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text(stringResource(R.string.pair_verify_different)) }
        }
    }
}

private fun platformLabel(platform: DevicePlatform): Int = when (platform) {
    DevicePlatform.Android -> R.string.platform_android
    DevicePlatform.Windows -> R.string.platform_windows
    DevicePlatform.Other -> R.string.platform_other
}

/** "Valable encore 1:48" while the core's estimate says there is time left; nothing otherwise: it is a hint, never a cut-off. */
@Composable
internal fun ValidityCountdown(deadlineElapsedMs: Long, modifier: Modifier = Modifier) {
    val remaining = rememberCountdownMs(deadlineElapsedMs)
    if (remaining > 0) {
        Text(
            stringResource(R.string.pair_verify_validity, Countdown.format(remaining)),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = modifier,
        )
    }
}

/**
 * Six digits in boxes, a gap after the third: JetBrains Mono 28 semibold, tabular digits (charter). It is drawn on a secure window and
 * read aloud by the screen reader as two groups of three; it is never put in a string that leaves this function.
 */
@Composable
internal fun VerificationCodeBoxes(code: VerificationCode) {
    val description = stringResource(R.string.pair_verify_code_description, code.grouped)
    Row(
        Modifier.fillMaxWidth().semantics { contentDescription = description },
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        code.digits.forEachIndexed { index, digit ->
            if (index == 3) Spacer(Modifier.width(8.dp))
            Surface(
                shape = TomiTheme.shapes.field,
                color = MaterialTheme.colorScheme.surface,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                modifier = Modifier.width(44.dp).height(60.dp),
            ) {
                Box(contentAlignment = Alignment.Center) {
                    Text(
                        digit.toString(),
                        style = MaterialTheme.typography.titleLarge.copy(
                            fontFamily = TomiFonts.mono,
                            fontSize = 28.sp,
                            fontWeight = FontWeight.SemiBold,
                            fontFeatureSettings = TomiFonts.TABULAR_DIGITS,
                        ),
                    )
                }
            }
        }
    }
}

/** Step 3: one yes/no choice. No per-type checkboxes. Only asked when the other device offers a history. */
@Composable
private fun HistoryContent(viewModel: PairingViewModel) {
    val context = LocalContext.current
    val awaiting = viewModel.awaiting
    val count = awaiting?.historyItemCount ?: 0
    val bytes = awaiting?.historyBytes ?: 0L
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.pair_history_title), style = MaterialTheme.typography.titleLarge)
            Text(
                pluralStringResource(R.plurals.pair_history_body, count, count),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (bytes > 0) {
                Text(
                    stringResource(R.string.pair_history_size, Formatter.formatShortFileSize(context, bytes)),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(8.dp))
            Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                HistoryChoice(
                    selected = viewModel.importHistory,
                    title = R.string.pair_history_yes,
                    hint = R.string.pair_history_yes_hint,
                    onSelect = { viewModel.onImportHistoryChange(true) },
                )
                HistoryChoice(
                    selected = !viewModel.importHistory,
                    title = R.string.pair_history_no,
                    hint = R.string.pair_history_no_hint,
                    onSelect = { viewModel.onImportHistoryChange(false) },
                )
            }
            Text(
                stringResource(R.string.pair_history_footer),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        BottomActions {
            Button(
                onClick = viewModel::confirmHistory,
                enabled = !viewModel.busy,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text(stringResource(R.string.action_continue)) }
        }
    }
}

/**
 * Step 4: this side has confirmed. The space key arrives only once the person on the other device has confirmed too; the wait can last
 * as long as the channel lives, so the way out stays visible.
 */
@Composable
private fun WaitingContent(viewModel: PairingViewModel) {
    val peerName = viewModel.awaiting?.peer?.name.orEmpty()
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        ) {
            Text(stringResource(R.string.pair_waiting_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.pair_waiting_body, peerName),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(8.dp))
            // Charter 6.3: progress is linear, never a spinner.
            LinearProgressIndicator(Modifier.fillMaxWidth())
            ValidityCountdown(viewModel.validUntilElapsedMs, Modifier.fillMaxWidth().padding(top = 8.dp))
        }
        BottomActions {
            TextButton(
                onClick = viewModel::leave,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text(stringResource(R.string.action_cancel)) }
        }
    }
}

@Composable
private fun HistoryChoice(selected: Boolean, title: Int, hint: Int, onSelect: () -> Unit) {
    Surface(
        shape = TomiTheme.shapes.card,
        color = if (selected) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surface,
        border = BorderStroke(
            if (selected) 2.dp else 1.dp,
            if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant,
        ),
        modifier = Modifier
            .fillMaxWidth()
            .clip(TomiTheme.shapes.card)
            .selectable(selected = selected, onClick = onSelect, role = Role.RadioButton),
    ) {
        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            RadioButton(selected = selected, onClick = null, modifier = Modifier.padding(top = 2.dp))
            Column {
                Text(stringResource(title), style = MaterialTheme.typography.labelLarge)
                Text(
                    stringResource(hint),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }
    }
}

@Composable
internal fun BottomActions(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 16.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) { content() }
}

/** The device is in the space: the name it joined with, read from the core. */
@Composable
private fun JoinedContent(onFinished: () -> Unit) {
    val localName = LocalContext.current.appContainer().core.localDevice.collectAsStateWithLifecycle().value.name
    PairedContent(
        title = stringResource(R.string.pair_done_title),
        body = stringResource(R.string.pair_done_body, localName),
        onFinished = onFinished,
    )
}

/** Success screen. Asks for notifications here, right after pairing, and never at first launch. */
@Composable
internal fun PairedContent(title: String, body: String, onFinished: () -> Unit) {
    val context = LocalContext.current
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { onFinished() }
    val alreadyGranted = ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    Column(Modifier.fillMaxSize().padding(24.dp)) {
        Column(
            Modifier.weight(1f).fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(Icons.Outlined.Check, contentDescription = null, modifier = Modifier.size(72.dp), tint = TomiTheme.colors.success)
            Text(title, style = MaterialTheme.typography.titleLarge)
            Text(
                body,
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (!alreadyGranted) {
                Spacer(Modifier.height(12.dp))
                Text(stringResource(R.string.pair_notifications_rationale), style = MaterialTheme.typography.bodyLarge)
            }
        }
        BottomActions {
            if (alreadyGranted) {
                Button(onClick = onFinished, shape = TomiTheme.shapes.button, modifier = Modifier.fillMaxWidth().height(48.dp)) {
                    Text(stringResource(R.string.action_continue))
                }
            } else {
                Button(
                    onClick = { launcher.launch(Manifest.permission.POST_NOTIFICATIONS) },
                    shape = TomiTheme.shapes.button,
                    modifier = Modifier.fillMaxWidth().height(48.dp),
                ) {
                    Icon(Icons.Outlined.Notifications, contentDescription = null, modifier = Modifier.size(20.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(stringResource(R.string.pair_notifications_enable))
                }
                TextButton(
                    onClick = onFinished,
                    shape = TomiTheme.shapes.button,
                    modifier = Modifier.fillMaxWidth().height(48.dp),
                ) { Text(stringResource(R.string.action_later)) }
            }
        }
    }
}
