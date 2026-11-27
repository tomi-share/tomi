package eu.shaffe.tomi.ui.feed

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Article
import androidx.compose.material.icons.automirrored.outlined.InsertDriveFile
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.outlined.OpenInNew
import androidx.compose.material.icons.automirrored.outlined.TextSnippet
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.DoneAll
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Link
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PushPin
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Unarchive
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DeliveryStatus
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.ItemKind
import eu.shaffe.tomi.core.ReadPolicy
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.isOutgoing
import eu.shaffe.tomi.core.kind
import eu.shaffe.tomi.core.slotOf
import eu.shaffe.tomi.share.FileDownloads
import eu.shaffe.tomi.share.FileSharing
import eu.shaffe.tomi.share.ItemSharing
import eu.shaffe.tomi.ui.common.deliveryLabel
import eu.shaffe.tomi.ui.common.formatRemaining
import eu.shaffe.tomi.ui.common.rememberNow
import eu.shaffe.tomi.ui.common.formatTimestamp
import eu.shaffe.tomi.ui.render.CARD_BUDGET
import eu.shaffe.tomi.ui.render.ChunkedText
import eu.shaffe.tomi.ui.render.PreviewKind
import eu.shaffe.tomi.ui.render.RenderMode
import eu.shaffe.tomi.ui.render.RenderModes
import eu.shaffe.tomi.ui.render.RichMarkdownView
import eu.shaffe.tomi.ui.render.head
import eu.shaffe.tomi.ui.render.preview
import eu.shaffe.tomi.ui.render.SandboxedHtmlView
import eu.shaffe.tomi.ui.theme.TomiFonts
import eu.shaffe.tomi.ui.theme.TomiTheme

/** Everything a card (or the toolbar of the detail screen) can ask the screen to do. */
class CardActions(
    /** A tap landed on the card, whatever it then does: marks an unread item read (never a hidden one, see the view model). */
    val onCardOpened: () -> Unit = {},
    /** Navigates to the detail screen (`item/{id}`). */
    val onOpenDetail: () -> Unit = {},
    /** Opens the single attachment with the system viewer; falls back to the detail when it cannot be opened. */
    val onOpenAttachment: () -> Unit = {},
    /** Shows the image of this SLOT (`Attachment.slot`) full screen in Tomi's internal viewer (biometric gate first for a hidden item). */
    val onOpenImage: (slot: Int) -> Unit = {},
    /** Shows the code file of this SLOT in Tomi's code screen (biometric gate first for a hidden item). */
    val onOpenCodeFile: (slot: Int) -> Unit = {},
    /** Opens the file of this SLOT (PDF, archive...) with the system viewer (biometric gate first for a hidden item). */
    val onOpenFile: (slot: Int) -> Unit = {},
    val onReveal: () -> Unit = {},
    val onHide: () -> Unit = {},
    val onCopy: (RenderMode?) -> Unit = {},
    val onCopyLink: () -> Unit = {},
    val onOpenLink: () -> Unit = {},
    val onShare: () -> Unit = {},
    val onTogglePin: () -> Unit = {},
    /** Archives the item, or un-archives an archived one (the same call as the swipe, through the core contract). */
    val onToggleArchive: () -> Unit = {},
    val onDeleteLocal: () -> Unit = {},
    val onDeleteEverywhere: () -> Unit = {},
    /** Removes ONE attachment (by its slot) from the item: this device only, or everywhere. The detail's file rows call it. */
    val onDeleteAttachment: (slot: Int, everywhere: Boolean) -> Unit = { _, _ -> },
    /** "Partager" of ONE file (by its slot), from the "⋮" of its row: the file alone, without the description (gate first for a hidden item). */
    val onShareFile: (slot: Int) -> Unit = {},
    /** "Télécharger" of ONE file (by its slot) to `Downloads/Tomi`, from the "⋮" of its row. Never for a hidden item. */
    val onDownloadFile: (slot: Int) -> Unit = {},
    /** The relay supports "remove this file everywhere" (otherwise the rows only offer "from this device"). */
    val attachmentRemovalEverywhere: Boolean = false,
    val onRenderMode: (RenderMode) -> Unit = {},
)

private const val MASK = "••••••••••"

/** border.width.secret-accent of the charter. */
private val SECRET_ACCENT_WIDTH = 3.dp

private fun CardTapAction.labelRes(): Int? = when (this) {
    CardTapAction.OpenLink, CardTapAction.OpenAttachment -> R.string.action_open
    CardTapAction.OpenImage -> R.string.action_view_image
    CardTapAction.Reveal -> R.string.action_reveal
    CardTapAction.Hide -> R.string.action_hide
    CardTapAction.OpenCodeFile -> R.string.action_view_code
    CardTapAction.OpenDetail -> R.string.action_details
    CardTapAction.None -> null
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ItemCard(
    item: TomiItem,
    localDeviceId: String,
    isOnline: Boolean,
    revealed: Boolean,
    preferredMode: RenderMode?,
    actions: CardActions,
    modifier: Modifier = Modifier,
    expanded: Boolean = false,
    /** The "..." menu (also opened by a long press). The detail screen offers the same actions in its toolbar instead. */
    showMenu: Boolean = true,
) {
    val context = LocalContext.current
    val haptics = LocalHapticFeedback.current
    val shape = TomiTheme.shapes.card
    val kind = item.kind()
    val outgoing = item.isOutgoing(localDeviceId)
    val unread = !item.read && !outgoing
    val textMode = item.text?.let { RenderModes.resolve(it, preferredMode) }
    var confirmDelete by remember { mutableStateOf(false) }
    var menuOpen by remember { mutableStateOf(false) }
    val secretColor = TomiTheme.colors.secret

    // What the card measured about itself: is the title / the text cut by the card limits? Reset when the displayed
    // content changes (another render mode, hidden <-> revealed), then filled again by the next layout.
    var titleCut by remember(item.id, textMode, revealed) { mutableStateOf(false) }
    var bodyCut by remember(item.id, textMode, revealed) { mutableStateOf(false) }
    val overflow = if (expanded) CardOverflow() else CardOverflow(title = titleCut, body = bodyCut)
    val tapAction = CardTap.actionFor(item, revealed, overflow)
    val showReadMore = !expanded && CardTap.showsReadMore(item, revealed, overflow)
    val tapLabel = tapAction.labelRes()?.let { stringResource(it) }
    val hasInlineCopy = showMenu && CardTap.showsCopyButton(item)
    val menuEntries = ItemMenuLogic.entries(
        item,
        revealed,
        canArchive = ArchiveSwipe.canArchive(item, localDeviceId),
        hasInlineCopy = hasInlineCopy,
        canShare = ItemSharing.canShare(item) || FileSharing.canShare(item),
    )
    // TalkBack cannot reach the menu by a long press nor swipe: every entry is also a custom action of the card. The archive one is
    // the swipe wrapper's own when the card swipes (the feed list), so it is not offered twice.
    val a11yActions = if (showMenu && !expanded) {
        cardAccessibilityActions(
            item, menuEntries, hasInlineCopy, archiveIsSwipe = ArchiveSwipe.swipeAction(item, localDeviceId) != null,
            textMode, actions, onDelete = { confirmDelete = true },
        )
    } else {
        emptyList()
    }

    Card(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            // The expanded card of the detail screen is not a button.
            .then(
                if (expanded) {
                    Modifier
                } else {
                    Modifier.semantics { customActions = a11yActions }.combinedClickable(
                        onClickLabel = tapLabel,
                        onClick = { actions.handleTap(tapAction, singleSlot = if (item.attachments.isEmpty()) 0 else item.attachments.slotOf(0)) },
                        onLongClick = if (showMenu) {
                            {
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                menuOpen = true
                            }
                        } else {
                            null
                        },
                    )
                },
            ),
        shape = shape,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        // Hidden items have a plum edge on the left (border.width.secret-accent). It is DRAWN behind the content (clipped by the
        // rounded card), not laid out: the content of a hidden card sits exactly where the one of a normal card does.
        Box(if (item.secret) Modifier.drawBehind { drawRect(secretColor, size = Size(SECRET_ACCENT_WIDTH.toPx(), size.height)) } else Modifier) {
            Column(
                // The header buttons are 48 dp tall touch targets and carry the top margin themselves.
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = if (showMenu) 0.dp else 8.dp, bottom = 16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                // Header: type icon, source and time, pin, unread dot, menu.
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Icon(
                        if (item.secret) Icons.Outlined.VisibilityOff else kind.icon(),
                        contentDescription = kind.label(),
                        modifier = Modifier.size(20.dp),
                        tint = if (item.secret) secretColor else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    val source = if (outgoing) stringResource(R.string.source_this_phone) else item.sourceDevice.name
                    Text(
                        "$source · ${formatTimestamp(context, item.createdAt)}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    if (item.pinned) {
                        Icon(
                            Icons.Filled.PushPin,
                            contentDescription = stringResource(R.string.pinned_badge),
                            modifier = Modifier.size(16.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    if (item.isShownAsArchived()) {
                        Icon(
                            Icons.Outlined.Archive,
                            contentDescription = stringResource(R.string.archived_badge),
                            modifier = Modifier.size(16.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    if (unread) {
                        val unreadDescription = stringResource(R.string.unread_badge)
                        Box(
                            Modifier
                                .size(8.dp)
                                .clip(CircleShape)
                                .background(MaterialTheme.colorScheme.primary)
                                .semantics { contentDescription = unreadDescription },
                        )
                    }
                    // Explicit copy icon on a text card (a tap on the text no longer copies). Same action as "Copier" of the menu.
                    if (showMenu && CardTap.showsCopyButton(item)) {
                        IconButton(onClick = { actions.onCopy(textMode) }, modifier = Modifier.size(48.dp)) {
                            Icon(
                                Icons.Outlined.ContentCopy,
                                contentDescription = stringResource(R.string.action_copy),
                                modifier = Modifier.size(20.dp),
                                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                    if (showMenu) {
                        Box {
                            IconButton(onClick = { menuOpen = true }, modifier = Modifier.size(48.dp)) {
                                Icon(
                                    Icons.Outlined.MoreVert,
                                    contentDescription = stringResource(R.string.action_more),
                                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                            ItemMenu(
                                expanded = menuOpen,
                                onDismiss = { menuOpen = false },
                                item = item,
                                entries = menuEntries,
                                textMode = textMode,
                                actions = actions,
                                onConfirmDelete = { confirmDelete = true },
                            )
                        }
                    }
                }

                Column(Modifier.padding(end = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    when (val content = item.content) {
                        is ItemContent.Unreadable -> UnreadableBody(content.reason)
                        is ItemContent.Readable ->
                            if (content.secret) {
                                SecretBody(
                                    item, revealed, expanded, actions, showReadMore,
                                    onTitleCut = { titleCut = it }, onBodyCut = { bodyCut = it },
                                )
                            } else {
                                PlainBody(
                                    item, unread, textMode, expanded, actions, showReadMore,
                                    opensAttachment = !expanded && tapAction == CardTapAction.OpenAttachment,
                                    onTitleCut = { titleCut = it },
                                    onBodyCut = { bodyCut = it },
                                )
                            }
                    }
                    if (showReadMore) {
                        ReadMore(onClick = {
                            actions.onCardOpened()
                            actions.onOpenDetail()
                        })
                    }
                    if (item.isReadable) CardFooter(item, localDeviceId, isOnline, expanded)
                }
            }
        }
    }

    if (confirmDelete) {
        DeleteItemDialog(
            secret = item.secret,
            onDeleteLocal = {
                confirmDelete = false
                actions.onDeleteLocal()
            },
            onDeleteEverywhere = {
                confirmDelete = false
                actions.onDeleteEverywhere()
            },
            onDismiss = { confirmDelete = false },
        )
    }
}

/**
 * Confirmation of every "delete from this device" (an item, a file, an image). The core forgets the item here for good and there is no
 * "Annuler" banner to fall back on, so the entry is not one tap away from being irreversible (it is also drawn in the error colour).
 */
@Composable
fun DeleteLocalDialog(onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        shape = TomiTheme.shapes.menu,
        title = { Text(stringResource(R.string.dialog_delete_local_title)) },
        text = { Text(stringResource(R.string.dialog_delete_local_message)) },
        confirmButton = {
            TextButton(onClick = onConfirm) {
                Text(stringResource(R.string.action_delete_local), color = MaterialTheme.colorScheme.error)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

/**
 * "Supprimer…" of an item (the card menu and the detail): ONE entry that asks where. Everywhere is the signed, irreversible deletion
 * for every device; this device only forgets the item here for good (the others keep it). Both say so, in the words of the two
 * earlier dialogs; the lesser one is not in the error colour.
 */
@Composable
fun DeleteItemDialog(secret: Boolean, onDeleteLocal: () -> Unit, onDeleteEverywhere: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        shape = TomiTheme.shapes.menu,
        title = { Text(stringResource(R.string.dialog_delete_item_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(stringResource(if (secret) R.string.dialog_delete_everywhere_secret else R.string.dialog_delete_everywhere_message))
                Text(stringResource(R.string.dialog_delete_local_message))
            }
        },
        confirmButton = {
            Column(horizontalAlignment = Alignment.End) {
                TextButton(onClick = onDeleteEverywhere) {
                    Text(stringResource(R.string.action_delete_everywhere), color = MaterialTheme.colorScheme.error)
                }
                TextButton(onClick = onDeleteLocal) { Text(stringResource(R.string.action_delete_local)) }
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

/** The custom accessibility actions of a card (TalkBack's actions menu): the same entries as its "⋮" menu, plus its copy icon. */
@Composable
private fun cardAccessibilityActions(
    item: TomiItem,
    entries: List<ItemMenuEntry>,
    hasInlineCopy: Boolean,
    archiveIsSwipe: Boolean,
    textMode: RenderMode?,
    actions: CardActions,
    onDelete: () -> Unit,
): List<CustomAccessibilityAction> {
    @Composable
    fun action(label: Int, block: () -> Unit) = CustomAccessibilityAction(stringResource(label)) {
        block()
        true
    }
    val linkOnly = item.url != null && item.text == null
    return buildList {
        if (hasInlineCopy) add(action(if (linkOnly) R.string.action_copy_link else R.string.action_copy) { actions.onCopy(textMode) })
        entries.forEach { entry ->
            when (entry) {
                ItemMenuEntry.Copy -> add(action(if (linkOnly) R.string.action_copy_link else R.string.action_copy) { actions.onCopy(textMode) })
                ItemMenuEntry.OpenLink -> add(action(R.string.action_open, actions.onOpenLink))
                ItemMenuEntry.CopyLink -> add(action(R.string.action_copy_link, actions.onCopyLink))
                ItemMenuEntry.Share -> add(action(R.string.action_share, actions.onShare))
                ItemMenuEntry.Pin -> add(action(if (item.pinned) R.string.action_unpin else R.string.action_pin, actions.onTogglePin))
                ItemMenuEntry.Archive -> if (!archiveIsSwipe) {
                    add(action(if (item.isShownAsArchived()) R.string.action_unarchive else R.string.action_archive, actions.onToggleArchive))
                }
                ItemMenuEntry.Details -> add(
                    action(R.string.action_details) {
                        actions.onCardOpened()
                        actions.onOpenDetail()
                    },
                )
                ItemMenuEntry.Delete -> add(action(R.string.action_delete_ellipsis, onDelete))
            }
        }
    }
}

/** Menu of an item: the entries of [ItemMenuLogic] in order, then a divider and the single "Supprimer…". */
@Composable
private fun ItemMenu(
    expanded: Boolean,
    onDismiss: () -> Unit,
    item: TomiItem,
    entries: List<ItemMenuEntry>,
    textMode: RenderMode?,
    actions: CardActions,
    onConfirmDelete: () -> Unit,
) {
    fun run(block: () -> Unit): () -> Unit = {
        onDismiss()
        block()
    }
    // A link-only item copies its link with "Copy".
    val linkOnly = item.url != null && item.text == null
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        shape = TomiTheme.shapes.menu,
        containerColor = MaterialTheme.colorScheme.surface,
    ) {
        entries.forEach { entry ->
            when (entry) {
                ItemMenuEntry.Copy -> DropdownMenuItem(
                    text = { Text(stringResource(if (linkOnly) R.string.action_copy_link else R.string.action_copy)) },
                    leadingIcon = { Icon(if (linkOnly) Icons.Outlined.Link else Icons.Outlined.ContentCopy, contentDescription = null) },
                    onClick = run { actions.onCopy(textMode) },
                )
                ItemMenuEntry.OpenLink -> DropdownMenuItem(
                    text = { Text(stringResource(R.string.action_open)) },
                    leadingIcon = { Icon(Icons.AutoMirrored.Outlined.OpenInNew, contentDescription = null) },
                    onClick = run(actions.onOpenLink),
                )
                ItemMenuEntry.CopyLink -> DropdownMenuItem(
                    text = { Text(stringResource(R.string.action_copy_link)) },
                    leadingIcon = { Icon(Icons.Outlined.Link, contentDescription = null) },
                    onClick = run(actions.onCopyLink),
                )
                ItemMenuEntry.Share -> DropdownMenuItem(
                    text = { Text(stringResource(R.string.action_share)) },
                    leadingIcon = { Icon(Icons.Outlined.Share, contentDescription = null) },
                    onClick = run(actions.onShare),
                )
                ItemMenuEntry.Pin -> DropdownMenuItem(
                    text = { Text(stringResource(if (item.pinned) R.string.action_unpin else R.string.action_pin)) },
                    leadingIcon = { Icon(if (item.pinned) Icons.Filled.PushPin else Icons.Outlined.PushPin, contentDescription = null) },
                    onClick = run(actions.onTogglePin),
                )
                ItemMenuEntry.Archive -> {
                    val archived = item.isShownAsArchived()
                    DropdownMenuItem(
                        text = { Text(stringResource(if (archived) R.string.action_unarchive else R.string.action_archive)) },
                        leadingIcon = { Icon(if (archived) Icons.Outlined.Unarchive else Icons.Outlined.Archive, contentDescription = null) },
                        onClick = run(actions.onToggleArchive),
                    )
                }
                // The detail screen stays reachable for every item (full content, type, expiry, status), even when a tap on the
                // card does something else.
                ItemMenuEntry.Details -> DropdownMenuItem(
                    text = { Text(stringResource(R.string.action_details)) },
                    leadingIcon = { Icon(Icons.Outlined.Info, contentDescription = null) },
                    onClick = run {
                        actions.onCardOpened()
                        actions.onOpenDetail()
                    },
                )
                ItemMenuEntry.Delete -> {
                    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.action_delete_ellipsis)) },
                        leadingIcon = { Icon(Icons.Outlined.DeleteOutline, contentDescription = null) },
                        colors = MenuDefaults.itemColors(
                            textColor = MaterialTheme.colorScheme.error,
                            leadingIconColor = MaterialTheme.colorScheme.error,
                        ),
                        onClick = run(onConfirmDelete),
                    )
                }
            }
        }
    }
}

/** Shown when the card cuts the content: the tap on the card (and this button) opens the detail to read it all. */
@Composable
private fun ReadMore(onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        TextButton(
            onClick = onClick,
            shape = TomiTheme.shapes.button,
            contentPadding = PaddingValues(start = 12.dp, end = 4.dp),
            modifier = Modifier.height(40.dp),
        ) {
            Text(stringResource(R.string.action_read_more))
            Icon(Icons.AutoMirrored.Outlined.KeyboardArrowRight, contentDescription = null, modifier = Modifier.size(20.dp))
        }
    }
}

/** Delivery status and expiry, in one caption line. Nothing for a plain received item without expiry. */
@Composable
private fun CardFooter(item: TomiItem, localDeviceId: String, isOnline: Boolean, expanded: Boolean) {
    val status = DeliveryStatus.of(item, localDeviceId)
    // "Livré à tous" is left out of the feed cards (kept on the detail screen): see [CardFooterLogic].
    val showStatus = CardFooterLogic.showsDeliveryStatus(status, expanded)
    val remaining = formatRemaining(item.expiresAt, rememberNow(item.expiresAt))
    val parts = buildList {
        if (showStatus) add(deliveryLabel(status, isOnline))
        if (item.readPolicy == ReadPolicy.AfterFirstRead) {
            add(stringResource(R.string.expires_after_read))
        } else if (remaining != null) {
            add(stringResource(R.string.expires_in, remaining))
        }
    }
    if (parts.isEmpty()) return
    val colors = MaterialTheme.colorScheme
    val (icon, tint) = when {
        !showStatus -> null to colors.onSurfaceVariant
        status == DeliveryStatus.Queued -> Icons.Outlined.Schedule to TomiTheme.colors.warning
        status == DeliveryStatus.Failed -> Icons.Outlined.ErrorOutline to colors.error
        status is DeliveryStatus.Delivered && status.delivered == status.total -> Icons.Outlined.DoneAll to TomiTheme.colors.success
        status is DeliveryStatus.Delivered -> Icons.Outlined.DoneAll to colors.onSurfaceVariant
        status == DeliveryStatus.Sent -> Icons.Outlined.Check to colors.onSurfaceVariant
        else -> null to colors.onSurfaceVariant
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        if (icon != null) Icon(icon, contentDescription = null, modifier = Modifier.size(16.dp), tint = tint)
        Text(
            parts.joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = if (status == DeliveryStatus.Queued) TomiTheme.colors.warning else colors.onSurfaceVariant,
        )
    }
}

/**
 * Hidden ("Masqué") item. Closed: only the word "Masqué" and a block "•••••• Afficher", never the title, the text,
 * the link or the file names. Open: title, content, and "Masquer".
 */
@Composable
private fun SecretBody(
    item: TomiItem,
    revealed: Boolean,
    expanded: Boolean,
    actions: CardActions,
    showReadMore: Boolean,
    onTitleCut: (Boolean) -> Unit,
    onBodyCut: (Boolean) -> Unit,
) {
    val secretColor = TomiTheme.colors.secret
    Text(stringResource(R.string.label_masked), style = MaterialTheme.typography.labelLarge, color = secretColor)
    if (!revealed) {
        Surface(color = TomiTheme.colors.secretContainer, shape = TomiTheme.shapes.maskBlock) {
            Row(Modifier.padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    MASK,
                    style = MaterialTheme.typography.titleMedium,
                    fontFamily = TomiFonts.mono,
                    modifier = Modifier.weight(1f),
                )
                TextButton(
                    onClick = actions.onReveal,
                    shape = TomiTheme.shapes.button,
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onSurface),
                    modifier = Modifier.height(48.dp),
                ) {
                    Icon(Icons.Outlined.Visibility, contentDescription = null, modifier = Modifier.size(20.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(stringResource(R.string.action_reveal))
                }
            }
        }
        // File names, sizes, thumbnails and types of a hidden item are hidden too; only the count is shown.
        if (item.attachments.isNotEmpty()) HiddenFilesCount(item.attachments.size)
    } else {
        item.title?.let { title ->
            Text(
                title,
                style = MaterialTheme.typography.bodyLarge,
                fontWeight = FontWeight.Medium,
                maxLines = if (expanded) Int.MAX_VALUE else 2,
                overflow = TextOverflow.Ellipsis,
                onTextLayout = { onTitleCut(it.hasVisualOverflow) },
            )
        }
        // Explicit onSurface: the revealed secret must stay very readable (the default for surfaceVariant is the muted onSurfaceVariant).
        Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            contentColor = MaterialTheme.colorScheme.onSurface,
            shape = TomiTheme.shapes.maskBlock,
        ) {
            Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                // Deliberately not selectable: the system selection toolbar would copy the content to the clipboard without
                // the sensitive flag, without the 30 s clearing and without marking the item read. "Copy" goes through
                // SensitiveClipboard.
                item.text?.let { secretText ->
                    val secretStyle = MaterialTheme.typography.bodyLarge.copy(
                        fontFamily = TomiFonts.mono,
                        fontFeatureSettings = TomiFonts.MONO_SECRET_FEATURES,
                    )
                    if (expanded) {
                        // Revealed only (this branch is never composed while hidden): pieces of lines revealed while
                        // scrolling, in memory only.
                        ChunkedText(secretText.plain, secretStyle)
                    } else {
                        // Revealed only: the card lays out its first lines, never the whole secret (in memory, never stored).
                        val shown = remember(secretText.plain) { cardHead(secretText.plain) }
                        Text(
                            shown.markdown,
                            style = secretStyle,
                            maxLines = 6,
                            overflow = TextOverflow.Ellipsis,
                            onTextLayout = { onBodyCut(it.hasVisualOverflow || shown.truncated) },
                        )
                    }
                }
                item.url?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodyMedium.copy(fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
                        fontFamily = TomiFonts.mono,
                    )
                }
                // Shown in clear, so its images may be opened; the viewer asks the biometric gate again (2-minute window).
                // The block is already surface-alt: tiles go to surface. No thumbnail (as before), so no mosaic either.
                AttachmentList(
                    item.attachments, showThumbnails = false, expanded = expanded,
                    onAltBackground = true, moreAsButton = !showReadMore,
                    onOpenImage = actions.openImageAction(), onOpenCodeFile = actions.openCodeFileAction(),
                    onOpenFile = actions.openFileAction(),
                    onOpenDetail = actions.openDetailAction(),
                    fileRowActions = actions.fileRowActionsFor(item, expanded),
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    val length = item.text?.plain?.length
                    if (length != null) {
                        Text(
                            pluralStringResource(R.plurals.characters_count, length, length),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Spacer(Modifier.weight(1f))
                    TextButton(
                        onClick = actions.onHide,
                        shape = TomiTheme.shapes.button,
                        colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onSurfaceVariant),
                    ) {
                        Icon(Icons.Outlined.VisibilityOff, contentDescription = null, modifier = Modifier.size(20.dp))
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.action_hide))
                    }
                }
            }
        }
    }
}

/** Item the core keeps but cannot show (unknown version, decryption failure): explained, never hidden, never a crash. */
@Composable
private fun UnreadableBody(reason: UnreadableReason) {
    Text(
        stringResource(
            when (reason) {
                UnreadableReason.UpdateRequired -> R.string.unreadable_update_required
                UnreadableReason.DecryptionFailed -> R.string.unreadable_decryption_failed
            },
        ),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PlainBody(
    item: TomiItem,
    unread: Boolean,
    mode: RenderMode?,
    expanded: Boolean,
    actions: CardActions,
    showReadMore: Boolean,
    /** A tap on the card opens the single generic attachment with the system viewer (its tile shows the outgoing arrow). */
    opensAttachment: Boolean,
    onTitleCut: (Boolean) -> Unit,
    onBodyCut: (Boolean) -> Unit,
) {
    val text = item.text
    // The text of a card is selectable (long press on the text); the long press elsewhere on the card still opens the menu.
    // PlainBody is never a hidden item: its selection toolbar would copy to the clipboard without the sensitive flag.
    SelectionContainer {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            item.title?.let { title ->
                Text(
                    title,
                    style = MaterialTheme.typography.bodyLarge,
                    fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Medium,
                    maxLines = if (expanded) Int.MAX_VALUE else 2,
                    overflow = TextOverflow.Ellipsis,
                    onTextLayout = { onTitleCut(it.hasVisualOverflow) },
                )
            }
            if (text != null && mode != null) {
                val modes = RenderModes.available(text)
                if (modes.size > 1) {
                    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                        modes.forEachIndexed { index, candidate ->
                            SegmentedButton(
                                selected = candidate == mode,
                                onClick = { actions.onRenderMode(candidate) },
                                shape = SegmentedButtonDefaults.itemShape(index, modes.size),
                            ) { Text(stringResource(candidate.labelRes())) }
                        }
                    }
                }
                val color = MaterialTheme.colorScheme.onSurface
                when (mode) {
                    // Styled rendering: Markdown through the native renderer, HTML through the sandboxed view, anything else as text.
                    RenderMode.Preview -> when (RenderModes.previewKind(text)) {
                        // The card guards, parses and renders a bounded preview, never the whole document (cut at a block
                        // boundary, see MarkdownBlocks); the detail and every action keep the full text.
                        PreviewKind.Markdown -> {
                            val source = text.markdown ?: text.plain
                            val shown = if (expanded) null else remember(source) { preview(source) }
                            val shownPlain = if (expanded) null else remember(text.plain) { cardHead(text.plain) }
                            RichMarkdownView(
                                markdown = shown?.markdown ?: source,
                                fallbackPlain = shownPlain?.markdown ?: text.plain,
                                selectable = true,
                                modifier = Modifier.fillMaxWidth(),
                                maxHeight = if (expanded) null else CARD_MAX_HEIGHT,
                                onClipped = { onBodyCut(it || shown?.truncated == true) },
                                truncated = shown?.truncated == true,
                            )
                        }
                        PreviewKind.Html -> SandboxedHtmlView(
                            html = text.html ?: text.plain,
                            textColor = color,
                            linkColor = MaterialTheme.colorScheme.primary,
                            maxHeight = if (expanded) EXPANDED_MAX_HEIGHT else CARD_MAX_HEIGHT,
                            modifier = Modifier.fillMaxWidth(),
                            onClipped = onBodyCut,
                            // In the feed a horizontal drag belongs to the card (swipe to archive); the detail still scrolls sideways.
                            horizontalScroll = expanded,
                        )
                        // The detail shows a long text as pieces of lines revealed while scrolling (ChunkedText).
                        PreviewKind.Plain -> if (expanded) {
                            ChunkedText(text.plain, MaterialTheme.typography.bodyMedium)
                        } else {
                            val shown = remember(text.plain) { cardHead(text.plain) }
                            Text(
                                shown.markdown,
                                style = MaterialTheme.typography.bodyMedium,
                                maxLines = 4,
                                overflow = TextOverflow.Ellipsis,
                                onTextLayout = { onBodyCut(it.hasVisualOverflow || shown.truncated) },
                            )
                        }
                    }
                    // The Markdown source, literally (#, -, ``` stay visible), in the monospace font.
                    RenderMode.Source -> {
                        val source = text.markdown ?: text.plain
                        val sourceStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = TomiFonts.mono, fontSize = 13.sp, lineHeight = 19.sp)
                        if (expanded) {
                            ChunkedText(source, sourceStyle)
                        } else {
                            val shown = remember(source) { cardHead(source) }
                            Text(
                                shown.markdown,
                                style = sourceStyle,
                                maxLines = SOURCE_MAX_LINES,
                                overflow = TextOverflow.Ellipsis,
                                onTextLayout = { onBodyCut(it.hasVisualOverflow || shown.truncated) },
                            )
                        }
                    }
                }
            }
        }
    }
    item.url?.let { url ->
        Text(
            url,
            style = MaterialTheme.typography.bodyMedium.copy(fontFeatureSettings = TomiFonts.MONO_EXACT_FEATURES),
            fontFamily = TomiFonts.mono,
            color = MaterialTheme.colorScheme.primary,
            maxLines = if (expanded) Int.MAX_VALUE else 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
    AttachmentList(
        item.attachments, showThumbnails = true, expanded = expanded,
        // Unread without a title: the names carry the weight the title would.
        emphasizeNames = unread && item.title == null,
        opensExternally = opensAttachment, moreAsButton = !showReadMore,
        onOpenImage = actions.openImageAction(), onOpenCodeFile = actions.openCodeFileAction(),
        onOpenFile = actions.openFileAction(),
        onOpenDetail = actions.openDetailAction(),
        fileRowActions = actions.fileRowActionsFor(item, expanded),
    )
}

/** The "⋮" menu (Partager, Télécharger, Supprimer) on the rows of the DETAIL of an item with several files; none on a card of the feed. */
private fun CardActions.fileRowActionsFor(item: TomiItem, expanded: Boolean): FileRowActions? =
    if (expanded && AttachmentRemovals.canRemoveFromBatch(item)) {
        FileRowActions(
            everywhere = attachmentRemovalEverywhere,
            secret = item.secret,
            canDownload = FileDownloads.canDownload(item),
            onShare = onShareFile,
            onDownload = onDownloadFile,
            onDelete = onDeleteAttachment,
        )
    } else {
        null
    }

/** "+N" of a mosaic / "N autres fichiers": the detail, and a tap that opens the card counts as opening it. */
private fun CardActions.openDetailAction(): () -> Unit = {
    onCardOpened()
    onOpenDetail()
}

/** Opening an image from its thumbnail counts as opening the card (marks an unread item read, like a tap on the card). */
private fun CardActions.openImageAction(): (Int) -> Unit = { index ->
    onCardOpened()
    onOpenImage(index)
}

/** Same for any other file row (PDF...): opens it with the system viewer. */
private fun CardActions.openFileAction(): (Int) -> Unit = { slot ->
    onCardOpened()
    onOpenFile(slot)
}

/** Same for a code file row. */
private fun CardActions.openCodeFileAction(): (Int) -> Unit = { index ->
    onCardOpened()
    onOpenCodeFile(index)
}

/** First lines of a text shown literally in a card (plain text, Markdown source, revealed secret): the card lays out no more. */
private fun cardHead(text: String) = head(text, CARD_BUDGET.maxLines, CARD_BUDGET.maxChars)

private val CARD_MAX_HEIGHT = 220.dp
private val EXPANDED_MAX_HEIGHT = 520.dp
private const val SOURCE_MAX_LINES = 8

internal fun ItemKind.icon(): ImageVector = when (this) {
    ItemKind.Unreadable -> Icons.Outlined.ErrorOutline
    ItemKind.Secret -> Icons.Outlined.VisibilityOff
    ItemKind.Image -> Icons.Outlined.Image
    ItemKind.File -> Icons.AutoMirrored.Outlined.InsertDriveFile
    ItemKind.Link -> Icons.Outlined.Link
    ItemKind.RichText -> Icons.AutoMirrored.Outlined.Article
    ItemKind.Text -> Icons.AutoMirrored.Outlined.TextSnippet
}

@Composable
internal fun ItemKind.label(): String = stringResource(
    when (this) {
        ItemKind.Unreadable -> R.string.kind_unreadable
        ItemKind.Secret -> R.string.kind_secret
        ItemKind.Image -> R.string.kind_image
        ItemKind.File -> R.string.kind_file
        ItemKind.Link -> R.string.kind_link
        ItemKind.RichText -> R.string.kind_rich_text
        ItemKind.Text -> R.string.kind_text
    },
)

private fun RenderMode.labelRes(): Int = when (this) {
    RenderMode.Preview -> R.string.render_preview
    RenderMode.Source -> R.string.render_markdown
}
