package eu.shaffe.tomi.ui.send

import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.InsertDriveFile
import androidx.compose.material.icons.automirrored.outlined.Send
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material.icons.outlined.Link
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import eu.shaffe.tomi.R
import eu.shaffe.tomi.send.readStreamMetadata
import eu.shaffe.tomi.share.LinkExtractor
import eu.shaffe.tomi.ui.common.TomiSnackbarHost
import eu.shaffe.tomi.ui.common.WithoutCopyActions
import eu.shaffe.tomi.ui.common.showFeedback
import eu.shaffe.tomi.ui.common.FlagSecureEffect
import eu.shaffe.tomi.ui.common.TomiFloatingToolbar
import eu.shaffe.tomi.ui.common.ToolbarFab
import eu.shaffe.tomi.ui.common.measureAsToolbar
import eu.shaffe.tomi.ui.common.rememberToolbarClearance
import eu.shaffe.tomi.ui.common.ToolbarIconButton
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * Compose zone opened by the Quick Settings tile (and by `tomi-share://send`). The clipboard is read ONLY inside the
 * click handler of the visible "Paste" button, never automatically. Paste and Attach live in the floating toolbar,
 * Send is its FAB.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SendScreen(viewModel: SendViewModel, onBack: () -> Unit) {
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    var showSecret by remember { mutableStateOf(false) }
    val clearance = rememberToolbarClearance()
    val recipients by viewModel.recipients.collectAsStateWithLifecycle()

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris: List<Uri> ->
        viewModel.onAttach(uris.map { readStreamMetadata(context, it) })
    }

    viewModel.notice?.let { resId ->
        val message = stringResource(resId)
        LaunchedEffect(resId) {
            snackbar.showFeedback(message)
            viewModel.consumeNotice()
        }
    }
    // Sent: back to the feed, where the item shows up with the "Sent / Undo" banner (Undo brings the text back here).
    LaunchedEffect(viewModel.leaveAfterSend) {
        if (viewModel.leaveAfterSend) {
            viewModel.consumeLeaveAfterSend()
            onBack()
        }
    }
    // Not only the text: the title and the names of the files of a hidden item are hidden content too.
    FlagSecureEffect(
        enabled = SecretCompose.needsSecureWindow(
            secret = viewModel.secret,
            text = viewModel.text,
            title = viewModel.title,
            hasAttachments = viewModel.attachments.isNotEmpty(),
        ),
    )
    // A revealed secret is masked again as soon as the screen leaves the foreground, or when the switch goes off.
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { showSecret = false }
    LaunchedEffect(viewModel.secret) { if (!viewModel.secret) showSecret = false }

    // contentWindowInsets includes the keyboard: the content area (fields AND floating toolbar) ends above the IME.
    // The imePadding() calls below stay as a safety net; the Scaffold insets are already consumed, so they add nothing.
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.send_title)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.Outlined.Close, contentDescription = stringResource(R.string.action_close))
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
    ) { padding ->
        Box(
            Modifier
                .screenContentPadding(padding)
                .fillMaxSize(),
        ) {
            Column(
                Modifier
                    .fillMaxSize()
                    .imePadding()
                    .verticalScroll(rememberScrollState())
                    .padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = clearance.bottomPadding),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                WithoutCopyActions(noCopy = !SecretCompose.allowsClipboardActions(viewModel.secret)) {
                    OutlinedTextField(
                        value = viewModel.title,
                        onValueChange = viewModel::onTitleChange,
                        label = { Text(stringResource(R.string.send_title_label)) },
                        singleLine = true,
                        shape = TomiTheme.shapes.field,
                        // The title of a hidden item is hidden too: the keyboard must not learn it (no suggestions, no auto-correct).
                        keyboardOptions = SecretCompose.keyboardOptions(viewModel.secret),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = viewModel.text,
                        onValueChange = viewModel::onTextChange,
                        label = { Text(stringResource(R.string.send_content_label)) },
                        minLines = 4,
                        maxLines = 10,
                        shape = TomiTheme.shapes.field,
                        visualTransformation = if (viewModel.secret && !showSecret) PasswordVisualTransformation() else VisualTransformation.None,
                        // Hidden content: password keyboard, no auto-correct, no suggestions, so the IME does not learn or store it.
                        keyboardOptions = SecretCompose.keyboardOptions(viewModel.secret),
                        trailingIcon = {
                            if (viewModel.secret) {
                                IconButton(onClick = { showSecret = !showSecret }) {
                                    Icon(
                                        if (showSecret) Icons.Outlined.VisibilityOff else Icons.Outlined.Visibility,
                                        contentDescription = stringResource(if (showSecret) R.string.action_hide else R.string.action_reveal),
                                    )
                                }
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }

                viewModel.attachments.forEachIndexed { index, file ->
                    AttachmentRow(
                        name = file.name ?: file.uri.substringAfterLast('/'),
                        onRemove = { viewModel.onRemoveAttachment(index) },
                    )
                }

                DetectionRow(viewModel, recipients)

                HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)

                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(stringResource(R.string.label_secret), style = MaterialTheme.typography.labelLarge)
                        Text(
                            stringResource(if (viewModel.secret) R.string.label_secret_on else R.string.label_secret_off),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Switch(checked = viewModel.secret, onCheckedChange = viewModel::onSecretChange, enabled = !viewModel.secretLocked)
                }

                Text(stringResource(R.string.expiry_label), style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(top = 4.dp))
                ExpiryChips(
                    secret = viewModel.secret,
                    hasAttachment = viewModel.attachments.isNotEmpty(),
                    selected = viewModel.expiry,
                    onSelect = viewModel::onExpiryChange,
                )
                if (viewModel.secret && viewModel.attachments.isEmpty()) {
                    Text(
                        stringResource(R.string.expiry_after_read_hint),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }

            viewModel.submission?.let { sent ->
                SentBanner(
                    skippedFiles = sent.skippedFiles,
                    deadline = sent.deadline,
                    onUndo = viewModel::undo,
                    // Options: take the message back and let the user adjust the switch and expiry above.
                    onOptions = viewModel::undo,
                    onTimeout = viewModel::onBannerTimeout,
                    modifier = Modifier.align(Alignment.BottomCenter).imePadding().padding(start = 16.dp, end = 16.dp, bottom = 96.dp),
                )
            }

            TomiSnackbarHost(
                snackbar,
                modifier = Modifier.align(Alignment.BottomCenter).imePadding().padding(bottom = 96.dp),
            )

            Box(Modifier.align(Alignment.BottomCenter).imePadding().padding(bottom = 16.dp)) {
                TomiFloatingToolbar(
                    modifier = Modifier.measureAsToolbar(clearance),
                    fab = {
                        ToolbarFab(
                            contentDescription = stringResource(R.string.action_send),
                            icon = Icons.AutoMirrored.Outlined.Send,
                            onClick = viewModel::send,
                            enabled = viewModel.canSend,
                        )
                    },
                ) {
                    // Visible paste button: the only place where the clipboard is read, and only on this click.
                    TextButton(
                        onClick = { pasteFromClipboard(context, viewModel) },
                        colors = androidx.compose.material3.ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onSurface),
                    ) {
                        Icon(Icons.Outlined.ContentPaste, contentDescription = null, modifier = Modifier.size(24.dp))
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.action_paste))
                    }
                    ToolbarIconButton(
                        icon = Icons.Outlined.AttachFile,
                        contentDescription = stringResource(R.string.action_attach),
                        onClick = { picker.launch(arrayOf("*/*")) },
                    )
                }
            }
        }
    }
}

/** Type detected under the field (link, text, file, hidden) and the devices it goes to. */
@Composable
private fun DetectionRow(viewModel: SendViewModel, recipients: List<String>) {
    val trimmed = viewModel.text.trim()
    val hasContent = trimmed.isNotEmpty() || viewModel.attachments.isNotEmpty()
    if (!hasContent) return
    val isLink = trimmed.isNotEmpty() && LinkExtractor.isValidHttpUrl(trimmed)
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (viewModel.secret) {
            DetectionChip(
                text = stringResource(R.string.detect_masked),
                icon = Icons.Outlined.VisibilityOff,
                container = TomiTheme.colors.secretContainer,
                content = TomiTheme.colors.secret,
            )
        } else {
            DetectionChip(
                text = stringResource(
                    when {
                        isLink -> R.string.detect_link
                        trimmed.isEmpty() -> R.string.detect_files
                        else -> R.string.detect_text
                    },
                ),
                icon = if (isLink) Icons.Outlined.Link else Icons.AutoMirrored.Outlined.InsertDriveFile,
                container = MaterialTheme.colorScheme.primaryContainer,
                content = MaterialTheme.colorScheme.primary,
            )
        }
        val caption = when {
            viewModel.secretHint == SecretHint.Heuristic -> stringResource(R.string.detect_masked_password)
            recipients.isNotEmpty() -> stringResource(R.string.detect_to_devices, recipients.joinToString(", "))
            else -> null
        }
        if (caption != null) {
            Text(
                caption,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
    when (viewModel.secretHint) {
        SecretHint.SourceSensitive -> HintText(R.string.hint_secret_sensitive)
        SecretHint.Heuristic -> HintText(R.string.hint_secret_heuristic)
        SecretHint.None -> Unit
    }
}

@Composable
private fun DetectionChip(text: String, icon: androidx.compose.ui.graphics.vector.ImageVector, container: androidx.compose.ui.graphics.Color, content: androidx.compose.ui.graphics.Color) {
    Surface(shape = CircleShape, color = container, contentColor = content) {
        Row(
            Modifier.padding(start = 8.dp, end = 12.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(18.dp))
            Text(text, style = MaterialTheme.typography.labelLarge)
        }
    }
}

@Composable
private fun AttachmentRow(name: String, onRemove: () -> Unit) {
    Surface(
        shape = TomiTheme.shapes.field,
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(Modifier.padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(Icons.AutoMirrored.Outlined.InsertDriveFile, contentDescription = null, modifier = Modifier.size(18.dp))
            // The whole name, in the proportional font like the tiles of the cards: it wraps, it is never cut.
            Text(name, style = MaterialTheme.typography.labelLarge, modifier = Modifier.weight(1f).padding(vertical = 12.dp))
            IconButton(onClick = onRemove) {
                Icon(Icons.Outlined.Close, contentDescription = stringResource(R.string.action_remove_attachment, name))
            }
        }
    }
}

@Composable
private fun HintText(resId: Int) {
    Text(stringResource(resId), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/**
 * The single place in the app that reads the clipboard. It is called from a click handler only.
 * Only text and HTML are taken from the clip: no URI is resolved, no coercion of other content types.
 */
private fun pasteFromClipboard(context: Context, viewModel: SendViewModel) {
    val manager = context.getSystemService(ClipboardManager::class.java)
    val clip = manager.primaryClip
    val first = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)
    val sensitive = clip?.description?.extras?.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE, false) == true
    viewModel.onPasted(first?.text?.toString(), first?.htmlText, sensitive)
}
