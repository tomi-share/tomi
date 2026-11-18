package eu.shaffe.tomi.ui.feed

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.OpenInNew
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.DeleteForever
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.Link
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PushPin
import androidx.compose.material.icons.outlined.Unarchive
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DeliveryStatus
import eu.shaffe.tomi.core.ReadPolicy
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.kind
import eu.shaffe.tomi.share.FileSharing
import eu.shaffe.tomi.share.ItemSharing
import eu.shaffe.tomi.ui.common.TomiSnackbarHost
import eu.shaffe.tomi.ui.common.TomiFloatingToolbar
import eu.shaffe.tomi.ui.common.ToolbarClearanceState
import eu.shaffe.tomi.ui.common.ToolbarFab
import eu.shaffe.tomi.ui.common.measureAsToolbar
import eu.shaffe.tomi.ui.common.rememberToolbarClearance
import eu.shaffe.tomi.ui.common.ToolbarIconButton
import eu.shaffe.tomi.ui.common.deliveryLabel
import eu.shaffe.tomi.ui.common.formatDateTime
import eu.shaffe.tomi.ui.common.formatPreciseDateTime
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.render.RenderModes
import eu.shaffe.tomi.ui.theme.TomiFonts
import eu.shaffe.tomi.ui.theme.TomiTheme

/**
 * Full-size card, reached from a notification, a `tomi-share://item/<id>` link, the "Détails" entry of a card menu, or a tap
 * on a card whose content is truncated (see [CardTap]).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ItemDetailScreen(
    itemId: String,
    viewModel: FeedViewModel,
    onBack: () -> Unit,
    onOpenImage: (slot: Int) -> Unit,
    onOpenCodeFile: (slot: Int) -> Unit,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    FeedEventsEffect(viewModel, snackbar)
    val item = state.allItems.firstOrNull { it.id == itemId }

    // Deleting the item from this screen closes it; an id that never existed keeps the "not found" message.
    var existed by remember { mutableStateOf(false) }
    LaunchedEffect(item) {
        if (item != null) existed = true else if (existed) onBack()
    }
    // The capability of the relay decides whether a file row offers "Supprimer le fichier partout".
    LaunchedEffect(viewModel) { viewModel.refreshAttachmentRemovalSupport() }
    val clearance = rememberToolbarClearance()

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
        topBar = {
            TopAppBar(
                title = {},
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_back))
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
            if (item == null) {
                if (state.loaded) {
                    Text(
                        stringResource(R.string.detail_not_found),
                        style = MaterialTheme.typography.bodyLarge,
                        modifier = Modifier.align(Alignment.Center).padding(16.dp),
                    )
                }
            } else {
                val actions = rememberCardActions(item, viewModel, onOpenDetail = {}, onOpenImage = onOpenImage, onOpenCodeFile = onOpenCodeFile)
                val revealed = item.id in state.revealedIds
                Column(
                    Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = clearance.bottomPadding),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    ItemCard(
                        item = item,
                        localDeviceId = state.localDeviceId,
                        isOnline = state.isOnline,
                        revealed = revealed,
                        preferredMode = state.preferredRenderMode,
                        actions = actions,
                        expanded = true,
                        showMenu = false,
                    )
                    if (item.secret) {
                        Text(
                            stringResource(if (revealed) R.string.detail_revealed_hint else R.string.detail_masked_hint),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    MetadataList(item, state)
                }
                DetailToolbar(item, revealed, state, actions, onBack, Modifier.align(Alignment.BottomCenter), clearance)
            }
            TomiSnackbarHost(
                snackbar,
                modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 96.dp),
            )
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun DetailToolbar(
    item: TomiItem,
    revealed: Boolean,
    state: FeedUiState,
    actions: CardActions,
    onBack: () -> Unit,
    modifier: Modifier,
    clearance: ToolbarClearanceState,
) {
    var moreMenu by remember { mutableStateOf(false) }
    var confirmEverywhere by remember { mutableStateOf(false) }
    val canCopy = item.text != null || item.url != null
    val textMode = item.text?.let { RenderModes.resolve(it, state.preferredRenderMode) }
    val canOpen = item.url != null && (!item.secret || revealed)
    val canCopyLinkSeparately = item.url != null && item.text != null && !item.secret
    val canShare = ItemSharing.canShare(item) || FileSharing.canShare(item)

    Box(modifier.navigationBarsPadding().padding(bottom = 16.dp)) {
        TomiFloatingToolbar(
            modifier = Modifier.measureAsToolbar(clearance),
            fab = if (canCopy) {
                {
                    ToolbarFab(
                        contentDescription = stringResource(R.string.action_copy),
                        icon = Icons.Outlined.ContentCopy,
                        onClick = { actions.onCopy(textMode) },
                    )
                }
            } else {
                null
            },
        ) {
            ToolbarIconButton(
                icon = if (item.pinned) Icons.Filled.PushPin else Icons.Outlined.PushPin,
                contentDescription = stringResource(if (item.pinned) R.string.action_unpin else R.string.action_pin),
                selected = item.pinned,
                onClick = actions.onTogglePin,
            )
            // Sharing is the main way out of the app: always one tap away, ahead of archive and delete.
            if (canShare) {
                ToolbarIconButton(
                    icon = Icons.Outlined.Share,
                    contentDescription = stringResource(R.string.action_share),
                    onClick = actions.onShare,
                )
            }
            if (ArchiveSwipe.canArchive(item, state.localDeviceId)) {
                val archived = item.isShownAsArchived()
                ToolbarIconButton(
                    icon = if (archived) Icons.Outlined.Unarchive else Icons.Outlined.Archive,
                    contentDescription = stringResource(if (archived) R.string.action_unarchive else R.string.action_archive),
                    selected = archived,
                    // Back to the list, where the banner "Archivé · Annuler" is shown (same as Windows).
                    onClick = {
                        actions.onToggleArchive()
                        onBack()
                    },
                )
            }
            Box {
                ToolbarIconButton(
                    icon = Icons.Outlined.MoreVert,
                    contentDescription = stringResource(R.string.action_more),
                    onClick = { moreMenu = true },
                )
                DropdownMenu(
                    expanded = moreMenu,
                    onDismissRequest = { moreMenu = false },
                    shape = TomiTheme.shapes.menu,
                    containerColor = MaterialTheme.colorScheme.surface,
                ) {
                    if (canOpen) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.action_open)) },
                            leadingIcon = { Icon(Icons.AutoMirrored.Outlined.OpenInNew, contentDescription = null) },
                            onClick = {
                                moreMenu = false
                                actions.onOpenLink()
                            },
                        )
                    }
                    if (canCopyLinkSeparately) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.action_copy_link)) },
                            leadingIcon = { Icon(Icons.Outlined.Link, contentDescription = null) },
                            onClick = {
                                moreMenu = false
                                actions.onCopyLink()
                            },
                        )
                    }
                    if (canOpen || canCopyLinkSeparately) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.action_delete_local)) },
                        leadingIcon = { Icon(Icons.Outlined.DeleteOutline, contentDescription = null) },
                        onClick = {
                            moreMenu = false
                            actions.onDeleteLocal()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.action_delete_everywhere)) },
                        leadingIcon = { Icon(Icons.Outlined.DeleteForever, contentDescription = null) },
                        colors = MenuDefaults.itemColors(
                            textColor = MaterialTheme.colorScheme.error,
                            leadingIconColor = MaterialTheme.colorScheme.error,
                        ),
                        onClick = {
                            moreMenu = false
                            confirmEverywhere = true
                        },
                    )
                }
            }
        }
    }

    if (confirmEverywhere) {
        DeleteEverywhereDialog(
            secret = item.secret,
            onConfirm = {
                confirmEverywhere = false
                actions.onDeleteEverywhere()
            },
            onDismiss = { confirmEverywhere = false },
        )
    }
}

/** Key / value list under the card: type, expiry, pin, status. Reveals nothing about the content of a hidden item. */
@Composable
private fun MetadataList(item: TomiItem, state: FeedUiState) {
    val rows = buildList {
        val kindLabel = when {
            !item.isReadable -> stringResource(R.string.kind_unreadable)
            item.secret && item.attachments.isNotEmpty() -> stringResource(R.string.kind_file)
            item.secret && item.url != null -> stringResource(R.string.kind_link)
            item.secret -> stringResource(R.string.kind_text)
            else -> item.kind().label()
        }
        add(stringResource(R.string.meta_type) to kindLabel)
        add(stringResource(R.string.meta_sent_at) to formatPreciseDateTime(item.createdAt))
        add(
            stringResource(R.string.meta_expiry) to when {
                item.readPolicy == ReadPolicy.AfterFirstRead -> stringResource(R.string.meta_after_read)
                item.expiresAt != null -> formatDateTime(item.expiresAt!!)
                else -> stringResource(R.string.meta_never)
            },
        )
        add(stringResource(R.string.meta_pinned) to stringResource(if (item.pinned) R.string.meta_yes else R.string.meta_no))
        add(stringResource(R.string.meta_status) to deliveryLabel(DeliveryStatus.of(item, state.localDeviceId), state.isOnline))
    }
    Surface(
        shape = TomiTheme.shapes.card,
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(horizontal = 16.dp)) {
            rows.forEachIndexed { index, (key, value) ->
                if (index > 0) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                Row(
                    Modifier.fillMaxWidth().padding(vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Text(key, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    // The value takes the remaining width and stays right-aligned, even when a long date wraps.
                    Row(
                        Modifier.weight(1f).padding(start = 16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.End),
                    ) {
                        if (key == stringResource(R.string.meta_pinned) && item.pinned) {
                            Icon(Icons.Filled.PushPin, contentDescription = null, modifier = Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Text(
                            value,
                            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = TomiFonts.ui),
                            textAlign = TextAlign.End,
                        )
                    }
                }
            }
        }
    }
}
