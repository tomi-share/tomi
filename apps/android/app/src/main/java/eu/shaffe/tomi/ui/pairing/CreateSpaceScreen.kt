package eu.shaffe.tomi.ui.pairing

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.PhoneAndroid
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import eu.shaffe.tomi.R
import eu.shaffe.tomi.ui.common.CenteredScrollColumn
import eu.shaffe.tomi.ui.common.FlagSecureEffect
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.theme.TomiFonts
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * "Crée ton espace": the installation key of the relay ("clé d'installation" on screen, the installation token of
 * spec/pairing-v1.md 10) and the name of this device. A secure window: the key is a secret. [onJoin] leads to the "join a space"
 * screen when the relay already has one.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CreateSpaceScreen(viewModel: CreateSpaceViewModel, onBack: () -> Unit, onJoin: () -> Unit, onFinished: () -> Unit) {
    FlagSecureEffect(enabled = true)
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
        topBar = {
            if (!viewModel.done) {
                // The title of the page is in the content; the bar only carries the way back.
                TopAppBar(
                    title = {},
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_back))
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
                )
            }
        },
    ) { padding ->
        Box(Modifier.screenContentPadding(padding).fillMaxSize()) {
            if (viewModel.done) {
                PairedContent(
                    title = stringResource(R.string.create_done_title),
                    body = stringResource(R.string.create_done_body, viewModel.deviceName.trim()),
                    onFinished = onFinished,
                )
            } else {
                CreateForm(viewModel, onJoin)
            }
        }
    }
}

@Composable
private fun CreateForm(viewModel: CreateSpaceViewModel, onJoin: () -> Unit) {
    val context = LocalContext.current
    var tokenVisible by remember { mutableStateOf(false) }
    var scanning by remember { mutableStateOf(false) }
    var cameraDenied by remember { mutableStateOf(false) }
    var helpOpen by remember { mutableStateOf(false) }
    var editingName by remember { mutableStateOf(false) }
    // The camera permission is asked only when the user taps the scan button, never at launch.
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        cameraDenied = !granted
        scanning = granted
    }
    // The scanner is closed by back before leaving the screen.
    BackHandler(enabled = scanning) { scanning = false }
    Column(Modifier.fillMaxSize()) {
        CenteredScrollColumn(
            Modifier.weight(1f).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(stringResource(R.string.create_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.create_intro),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (scanning && !viewModel.busy) {
                val previewDescription = stringResource(R.string.create_scan_preview_description)
                QrScanner(
                    onScanned = {
                        // The scanner is closed in every case; a valid key fills the field, nothing is submitted.
                        scanning = false
                        viewModel.onQrScanned(it)
                    },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(280.dp)
                        .clip(TomiTheme.shapes.menu)
                        .semantics { contentDescription = previewDescription },
                )
                TextButton(onClick = { scanning = false }, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.create_scan_cancel))
                }
            } else {
                val scanDescription = stringResource(R.string.create_scan_button_description)
                FilledTonalButton(
                    onClick = {
                        val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
                        cameraDenied = false
                        if (granted) scanning = true else launcher.launch(Manifest.permission.CAMERA)
                    },
                    enabled = !viewModel.busy,
                    shape = TomiTheme.shapes.button,
                    modifier = Modifier.fillMaxWidth().height(48.dp).semantics { contentDescription = scanDescription },
                ) {
                    Icon(Icons.Outlined.QrCodeScanner, contentDescription = null, modifier = Modifier.size(20.dp))
                    Text(stringResource(R.string.create_scan_button), modifier = Modifier.padding(start = 8.dp))
                }
            }
            if (cameraDenied) {
                Text(stringResource(R.string.create_camera_denied), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
            }
            OrSeparator()
            OutlinedTextField(
                value = viewModel.token,
                onValueChange = viewModel::onTokenChange,
                label = { Text(stringResource(R.string.create_token_label)) },
                singleLine = true,
                enabled = !viewModel.busy,
                shape = TomiTheme.shapes.field,
                textStyle = MaterialTheme.typography.bodyLarge.copy(
                    fontFamily = if (tokenVisible) TomiFonts.mono else FontFamily.Default,
                    fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES,
                ),
                visualTransformation = if (tokenVisible) VisualTransformation.None else PasswordVisualTransformation(),
                // A secret: no suggestion, no correction, nothing for the keyboard to learn.
                keyboardOptions = KeyboardOptions(
                    capitalization = KeyboardCapitalization.None,
                    autoCorrectEnabled = false,
                    keyboardType = KeyboardType.Password,
                    imeAction = ImeAction.Next,
                ),
                trailingIcon = {
                    IconButton(onClick = { tokenVisible = !tokenVisible }) {
                        Icon(
                            if (tokenVisible) Icons.Outlined.VisibilityOff else Icons.Outlined.Visibility,
                            contentDescription = stringResource(if (tokenVisible) R.string.action_hide else R.string.action_reveal),
                        )
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            )
            TextButton(onClick = { helpOpen = true }, modifier = Modifier.height(48.dp)) {
                Icon(Icons.Outlined.Info, contentDescription = null, modifier = Modifier.size(20.dp))
                Text(stringResource(R.string.create_where_button), modifier = Modifier.padding(start = 8.dp))
            }
            RelayField(
                value = viewModel.relayText,
                onValueChange = viewModel::onRelayChange,
                required = viewModel.relayRequired,
                configuredRelay = viewModel.configuredRelay,
                enabled = !viewModel.busy,
                imeAction = ImeAction.Done,
            )
            DeviceNameCard(viewModel.deviceName, enabled = !viewModel.busy, onEdit = { editingName = true })
            // The relay that "Créer" will use, in full: a relay read from a QR code, a link or a field is not trusted.
            viewModel.relayAuthority?.let { RelayCheckCard(it, R.string.create_relay_check_label, R.string.create_relay_check_hint) }
            if (viewModel.relayFromLink) {
                Text(
                    stringResource(R.string.create_relay_from_link),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            viewModel.error?.let {
                Text(stringResource(it), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
            }
            if (viewModel.spaceExists) {
                TextButton(onClick = onJoin) { Text(stringResource(R.string.create_join_instead)) }
            }
            if (viewModel.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        }
        BottomActions {
            Button(
                onClick = viewModel::submit,
                enabled = !viewModel.busy && viewModel.token.isNotBlank(),
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text(stringResource(R.string.create_submit)) }
        }
    }

    if (helpOpen) InstallKeyHelpSheet(onDismiss = { helpOpen = false })
    if (editingName) {
        DeviceNameDialog(
            current = viewModel.deviceName,
            onSave = {
                viewModel.onDeviceNameChange(it)
                editingName = false
            },
            onDismiss = { editingName = false },
        )
    }
}

/** A thin line, a label, a thin line. */
@Composable
private fun OrSeparator() {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        HorizontalDivider(Modifier.weight(1f))
        Text(
            stringResource(R.string.create_or_paste),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 12.dp),
        )
        HorizontalDivider(Modifier.weight(1f))
    }
}

/** The name of this device, prefilled by the app: a card with a pencil that opens the edition in a dialog. */
@Composable
private fun DeviceNameCard(name: String, enabled: Boolean, onEdit: () -> Unit) {
    Surface(
        shape = TomiTheme.shapes.card,
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.fillMaxWidth(),
    ) {
        ListItem(
            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
            leadingContent = {
                Surface(shape = CircleShape, color = MaterialTheme.colorScheme.primaryContainer, modifier = Modifier.size(40.dp)) {
                    Box(contentAlignment = Alignment.Center) {
                        Icon(Icons.Outlined.PhoneAndroid, contentDescription = null, tint = MaterialTheme.colorScheme.onPrimaryContainer)
                    }
                }
            },
            overlineContent = { Text(stringResource(R.string.create_name_label)) },
            headlineContent = { Text(name, style = MaterialTheme.typography.titleMedium) },
            trailingContent = {
                IconButton(onClick = onEdit, enabled = enabled) {
                    Icon(Icons.Outlined.Edit, contentDescription = stringResource(R.string.create_name_edit))
                }
            },
        )
    }
}

/** The name is checked when the space is created ([CreateSpaceViewModel.submit]); this only edits it. */
@Composable
private fun DeviceNameDialog(current: String, onSave: (String) -> Unit, onDismiss: () -> Unit) {
    var draft by remember { mutableStateOf(current) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.create_name_label)) },
        text = {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                singleLine = true,
                shape = TomiTheme.shapes.field,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = { TextButton(onClick = { onSave(draft) }) { Text(stringResource(R.string.action_save)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

/**
 * Where the installation key comes from. The command is shown, not copyable, and no example of a key is drawn: nothing here can be
 * mistaken for a real key.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun InstallKeyHelpSheet(onDismiss: () -> Unit) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
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
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.create_where_title), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.create_where_body),
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(stringResource(R.string.create_where_other_title), style = MaterialTheme.typography.titleMedium)
            Text(
                stringResource(R.string.create_where_run),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Surface(
                shape = TomiTheme.shapes.field,
                color = MaterialTheme.colorScheme.surfaceVariant,
                contentColor = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(
                    stringResource(R.string.create_where_command),
                    style = MaterialTheme.typography.bodyMedium.copy(fontFamily = TomiFonts.mono, fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
                    modifier = Modifier.padding(12.dp),
                )
            }
            Button(
                onClick = onDismiss,
                shape = TomiTheme.shapes.button,
                modifier = Modifier.fillMaxWidth().height(48.dp),
            ) { Text(stringResource(R.string.create_where_ok)) }
        }
    }
}
