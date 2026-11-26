package eu.shaffe.tomi.ui.feed

import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.InsertDriveFile
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.outlined.OpenInNew
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.Code
import androidx.compose.material.icons.outlined.DeleteForever
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.Download
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.findBySlot
import eu.shaffe.tomi.core.slotOf
import eu.shaffe.tomi.ui.code.CodeFiles
import eu.shaffe.tomi.ui.common.formatFileSize
import eu.shaffe.tomi.ui.render.RenderedHeights
import eu.shaffe.tomi.ui.theme.TomiFonts
import eu.shaffe.tomi.ui.theme.TomiTheme

/*
 * Attachments of a card (feed and detail). One attachment = ONE object with ONE touch
 * target: a tile (type badge, name, size, action on the right) or an image frame (thumbnail + caption). Rules in [AttachmentLogic].
 * What a tap does does not change (see [AttachmentRowAction] / [CardTap]): only the presentation.
 */

private val TileMinHeight = 56.dp
private val BadgeSize = 40.dp
private val FeedImageMaxHeight = 180.dp
private val DetailImageMaxHeight = 200.dp

/** Veil of the "+N" cell of a mosaic (token viewer-chip of the charter: black at 55 %, as in the image viewer). */
private val MosaicVeil = Color.Black.copy(alpha = 0.55f)

@Composable
internal fun AttachmentList(
    attachments: List<Attachment>,
    showThumbnails: Boolean,
    expanded: Boolean,
    /** Unread item without a title: the names are semi-bold, as the title of an unread card is. */
    emphasizeNames: Boolean = false,
    /** The list sits in a block that is already `surface-alt` (revealed hidden item): tiles go to `surface`. */
    onAltBackground: Boolean = false,
    /** A tap on the card opens the (single, generic) file with the system viewer: its tile shows the outgoing arrow. */
    opensExternally: Boolean = false,
    /** "N autres fichiers ›" button to the detail; false when "Lire la suite" already takes that place (a caption is shown instead). */
    moreAsButton: Boolean = true,
    /** Called with the SLOT of the attachment (not its position) when an image is tapped; images are not clickable when null. */
    onOpenImage: ((Int) -> Unit)? = null,
    /** Called with the SLOT of the attachment when a code file is tapped; code files are not clickable when null. */
    onOpenCodeFile: ((Int) -> Unit)? = null,
    /** Called with the SLOT of the attachment when any other file (PDF...) is tapped: it goes to the system viewer; such files are not clickable when null. */
    onOpenFile: ((Int) -> Unit)? = null,
    /** Opens the detail ("+N" of a mosaic, "N autres fichiers"); those are not shown when null. */
    onOpenDetail: (() -> Unit)? = null,
    /** "Supprimer ce fichier" next to each row (the detail of an item with several files); no such action when null. */
    fileRowActions: FileRowActions? = null,
) {
    if (attachments.isEmpty()) return
    // Slot of the attachment whose "everywhere" removal waits for its confirmation (which names the file).
    var confirmEverywhereSlot by remember { mutableStateOf<Int?>(null) }
    var confirmLocalSlot by remember { mutableStateOf<Int?>(null) }
    val context = LocalContext.current
    val viewLabel = stringResource(R.string.action_view_image)
    val codeLabel = stringResource(R.string.action_view_code)
    val openLabel = stringResource(R.string.action_open)
    val plan = AttachmentLogic.plan(attachments, expanded, showThumbnails)
    // Only the thumbnails the plan draws are decoded (a batch of 20 photos must not cost 20 decodes on every scroll back).
    val drawn: Set<Int> = when (plan) {
        is AttachmentLogic.Plan.Mosaic -> (0 until plan.shown).toSet()
        is AttachmentLogic.Plan.Items -> plan.entries.map { it.index }.toSet()
    }
    val bitmaps = attachments.mapIndexed { index, a -> rememberThumbnail(a, showThumbnails && index in drawn) }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val allImages = AttachmentLogic.isAllImages(attachments)
        val summary = if (AttachmentLogic.showsSummary(attachments)) {
            val count = attachments.size
            val size = formatFileSize(context, AttachmentLogic.totalSize(attachments))
            if (allImages) pluralStringResource(R.plurals.attachments_summary_images, count, count, size)
            else pluralStringResource(R.plurals.attachments_summary_files, count, count, size)
        } else {
            null
        }
        when (plan) {
            is AttachmentLogic.Plan.Mosaic -> {
                Mosaic(attachments, bitmaps, plan, onOpenImage?.let { open -> { index: Int -> open(attachments.slotOf(index)) } }, onOpenDetail)
                if (summary != null) AttachmentCaption(summary)
            }
            is AttachmentLogic.Plan.Items -> {
                // "4 fichiers · 4,9 Mo" above the attachments of the detail.
                if (expanded && summary != null) {
                    Text(
                        summary,
                        style = MaterialTheme.typography.bodySmall,
                        fontWeight = FontWeight.Medium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
                plan.entries.forEach { entry ->
                    val attachment = attachments[entry.index]
                    val bitmap = bitmaps[entry.index]
                    val action = AttachmentRowAction.of(attachment, onOpenImage != null, onOpenCodeFile != null, onOpenFile != null)
                    // The core is asked for the SLOT of the attachment: its position here is not it once a file was removed.
                    val slot = attachments.slotOf(entry.index)
                    val onClick: (() -> Unit)? = when (action) {
                        AttachmentRowAction.OpenImage -> ({ onOpenImage?.invoke(slot) })
                        AttachmentRowAction.OpenCodeFile -> ({ onOpenCodeFile?.invoke(slot) })
                        AttachmentRowAction.OpenExternal -> ({ onOpenFile?.invoke(slot) })
                        AttachmentRowAction.None -> null
                    }
                    val clickLabel = when (action) {
                        AttachmentRowAction.OpenImage -> viewLabel
                        AttachmentRowAction.OpenCodeFile -> codeLabel
                        AttachmentRowAction.OpenExternal -> openLabel
                        AttachmentRowAction.None -> null
                    }
                    val isFrame = entry.look == AttachmentLogic.Look.Frame && bitmap != null
                    // The menu of the row sits BESIDE the tile / the frame (not inside): the row stays one touch target, read as one element.
                    RowWithMenu(
                        fileRowActions = fileRowActions,
                        topAligned = isFrame,
                        onShare = { fileRowActions?.onShare?.invoke(slot) },
                        onDownload = { fileRowActions?.onDownload?.invoke(slot) },
                        onDeleteLocal = { confirmLocalSlot = slot },
                        onDeleteEverywhere = { confirmEverywhereSlot = slot },
                    ) {
                        if (isFrame && bitmap != null) {
                            ImageFrame(
                                attachment = attachment,
                                bitmap = bitmap,
                                // Detail: a lone image whole (the sender already crops it to 3:4 at most), several ones cropped at 200 dp.
                                maxHeight = if (!expanded) FeedImageMaxHeight else if (attachments.size == 1) null else DetailImageMaxHeight,
                                contentScale = if (expanded && attachments.size == 1) ContentScale.FillWidth else ContentScale.Crop,
                                full = expanded,
                                emphasized = emphasizeNames,
                                hasAction = action != AttachmentRowAction.None,
                                onClick = onClick,
                                onClickLabel = clickLabel,
                            )
                        } else {
                            FileTile(
                                attachment = attachment,
                                thumbnail = bitmap,
                                action = action,
                                external = opensExternally && action == AttachmentRowAction.None && attachments.size == 1,
                                full = expanded,
                                emphasized = emphasizeNames,
                                onAltBackground = onAltBackground,
                                onClick = onClick,
                                onClickLabel = clickLabel,
                            )
                        }
                    }
                }
                if (plan.more > 0) {
                    if (moreAsButton && onOpenDetail != null) {
                        MoreFiles(plan.more, onOpenDetail)
                    } else {
                        AttachmentCaption(pluralStringResource(R.plurals.more_files_caption, plan.more, plan.more))
                    }
                }
            }
        }
    }

    confirmLocalSlot?.let { slot ->
        // Same as below: nothing to confirm when the file has vanished meanwhile.
        if (attachments.findBySlot(slot) != null && fileRowActions != null) {
            DeleteLocalDialog(
                onConfirm = {
                    confirmLocalSlot = null
                    fileRowActions.onDelete(slot, false)
                },
                onDismiss = { confirmLocalSlot = null },
            )
        }
    }
    // The attachment may have vanished meanwhile (removed from another device): then there is nothing left to confirm.
    confirmEverywhereSlot?.let { slot ->
        val target = attachments.findBySlot(slot)
        // Nothing to confirm when the file has vanished meanwhile: the dialog is simply not shown (no state written while composing).
        if (target != null && fileRowActions != null) {
            DeleteFileEverywhereDialog(
                // A row is only on screen when its name may be (a hidden item is revealed first): the confirmation names the file.
                name = target.name,
                secret = fileRowActions.secret,
                onConfirm = {
                    confirmEverywhereSlot = null
                    fileRowActions.onDelete(slot, true)
                },
                onDismiss = { confirmEverywhereSlot = null },
            )
        }
    }
}

/**
 * What the detail gives the list of files to act on one of them ([AttachmentRemovals.canRemoveFromBatch]). [everywhere]: the relay
 * supports "remove everywhere" (otherwise only "from this device" is offered). [secret]: the item is a revealed hidden one (the
 * confirmation then says a device that displayed the file may have kept a copy). [canDownload]: "Télécharger" is offered (never for a
 * hidden item: no permanent clear copy outside the app). [onShare] / [onDownload] get the SLOT of the file.
 */
internal class FileRowActions(
    val everywhere: Boolean,
    val secret: Boolean,
    val canDownload: Boolean,
    val onShare: (slot: Int) -> Unit,
    val onDownload: (slot: Int) -> Unit,
    val onDelete: (slot: Int, everywhere: Boolean) -> Unit,
)

/** A row with, on its right, the menu "Partager / Télécharger / Supprimer" when [fileRowActions] is given; the row alone otherwise. */
@Composable
private fun RowWithMenu(
    fileRowActions: FileRowActions?,
    topAligned: Boolean,
    onShare: () -> Unit,
    onDownload: () -> Unit,
    onDeleteLocal: () -> Unit,
    onDeleteEverywhere: () -> Unit,
    row: @Composable () -> Unit,
) {
    if (fileRowActions == null) {
        row()
        return
    }
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = if (topAligned) Alignment.Top else Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f)) { row() }
        FileRowMenu(
            scopes = AttachmentRemovals.scopes(fileRowActions.everywhere),
            canDownload = fileRowActions.canDownload,
            onShare = onShare,
            onDownload = onDownload,
            onDeleteLocal = onDeleteLocal,
            onDeleteEverywhere = onDeleteEverywhere,
        )
    }
}

/**
 * "⋮" of a file row: "Partager" and "Télécharger" (the latter absent for a hidden item), a divider, then the destructive entries in the
 * error colour: "Supprimer le fichier de cet appareil" and, when the relay supports it, "Supprimer le fichier partout".
 */
@Composable
private fun FileRowMenu(
    scopes: List<RemovalScope>,
    canDownload: Boolean,
    onShare: () -> Unit,
    onDownload: () -> Unit,
    onDeleteLocal: () -> Unit,
    onDeleteEverywhere: () -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) {
            Icon(Icons.Outlined.MoreVert, contentDescription = stringResource(R.string.file_row_actions))
        }
        DropdownMenu(
            expanded = open,
            onDismissRequest = { open = false },
            shape = TomiTheme.shapes.menu,
            containerColor = MaterialTheme.colorScheme.surface,
        ) {
            DropdownMenuItem(
                text = { Text(stringResource(R.string.action_share)) },
                leadingIcon = { Icon(Icons.Outlined.Share, contentDescription = null) },
                onClick = {
                    open = false
                    onShare()
                },
            )
            // A hidden item cannot be saved out of Tomi (golden rule 3): no entry, no workaround.
            if (canDownload) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.file_download)) },
                    leadingIcon = { Icon(Icons.Outlined.Download, contentDescription = null) },
                    onClick = {
                        open = false
                        onDownload()
                    },
                )
            }
            // Same convention as the menu of the feed: a divider, then the destructive entries in the error colour.
            HorizontalDivider()
            if (RemovalScope.ThisDevice in scopes) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.file_delete_local)) },
                    leadingIcon = { Icon(Icons.Outlined.DeleteOutline, contentDescription = null) },
                    colors = MenuDefaults.itemColors(
                        textColor = MaterialTheme.colorScheme.error,
                        leadingIconColor = MaterialTheme.colorScheme.error,
                    ),
                    onClick = {
                        open = false
                        onDeleteLocal()
                    },
                )
            }
            if (RemovalScope.Everywhere in scopes) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.file_delete_everywhere)) },
                    leadingIcon = { Icon(Icons.Outlined.DeleteForever, contentDescription = null) },
                    colors = MenuDefaults.itemColors(
                        textColor = MaterialTheme.colorScheme.error,
                        leadingIconColor = MaterialTheme.colorScheme.error,
                    ),
                    onClick = {
                        open = false
                        onDeleteEverywhere()
                    },
                )
            }
        }
    }
}

/** Confirmation of "delete this file everywhere": it names the file (the item stays). */
@Composable
private fun DeleteFileEverywhereDialog(name: String, secret: Boolean, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        shape = TomiTheme.shapes.menu,
        title = { Text(stringResource(R.string.dialog_delete_file_everywhere_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.dialog_delete_file_everywhere_named, name))
                if (secret) Text(stringResource(R.string.dialog_delete_file_everywhere_secret))
            }
        },
        confirmButton = {
            TextButton(onClick = onConfirm) {
                Text(stringResource(R.string.file_delete_everywhere), color = MaterialTheme.colorScheme.error)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

@Composable
private fun rememberThumbnail(attachment: Attachment, enabled: Boolean): ImageBitmap? =
    remember(attachment.thumbnail, enabled) {
        if (!enabled) null else attachment.thumbnail?.let(ThumbnailCache::get)
    }

/**
 * Thumbnails already decoded, by the encoded bytes they come from: a card that comes back on screen (scrolled back, feed shown
 * again after a detail) does not decode its images again on the main thread. Keyed by the ARRAY itself, not its content: the
 * model keeps the same array as long as the item does not change (see `reuseUnchanged`). Memory only, bounded; never holds a
 * thumbnail of a hidden item, which is never decoded (see [AttachmentList], `showThumbnails`).
 */
private object ThumbnailCache {
    private const val MAX_BYTES = 16 * 1024 * 1024

    private val decoded = object : LruCache<ByteArray, ImageBitmap>(MAX_BYTES) {
        override fun sizeOf(key: ByteArray, value: ImageBitmap): Int = value.width * value.height * 4 + key.size
    }

    fun get(encoded: ByteArray): ImageBitmap? =
        decoded.get(encoded) ?: BitmapFactory.decodeByteArray(encoded, 0, encoded.size)?.asImageBitmap()?.also { decoded.put(encoded, it) }

    fun clear() = decoded.evictAll()
}

/** Forgets what the cards keep in memory about the items (decoded thumbnails, laid out heights): the space was left. */
internal fun clearRenderCaches() {
    ThumbnailCache.clear()
    RenderedHeights.clear()
}

@Composable
private fun AttachmentCaption(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** "N autres fichiers ›": text button aligned to the right like "Lire la suite", opens the detail. */
@Composable
private fun MoreFiles(count: Int, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        TextButton(
            onClick = onClick,
            shape = TomiTheme.shapes.button,
            contentPadding = PaddingValues(start = 12.dp, end = 4.dp),
            modifier = Modifier.height(40.dp),
        ) {
            Text(pluralStringResource(R.plurals.more_files, count, count))
            Icon(Icons.AutoMirrored.Outlined.KeyboardArrowRight, contentDescription = null, modifier = Modifier.size(20.dp))
        }
    }
}

/** Closed hidden item: only the count, with a paperclip. No name, no size, no thumbnail, no type (golden rule 3). */
@Composable
internal fun HiddenFilesCount(count: Int) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        Icon(
            Icons.Outlined.AttachFile,
            contentDescription = null,
            modifier = Modifier.size(16.dp),
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            pluralStringResource(R.plurals.secret_files_hidden, count, count),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

// ---------------------------------------------------------------- name

/**
 * A file name in the proportional font. [maxLines] null: the whole name, which wraps even without a space (detail). Otherwise the name is
 * cut BEFORE its extension so that it fits in [maxLines] lines ("Courrier banque fonds de rési….pdf"), measured in the width the text
 * really has ([AttachmentLogic.fitFileName]).
 */
@Composable
private fun FileName(name: String, style: TextStyle, maxLines: Int?, modifier: Modifier = Modifier) {
    if (maxLines == null) {
        Text(name, style = style, modifier = modifier)
        return
    }
    val measurer = rememberTextMeasurer()
    BoxWithConstraints(modifier) {
        if (!constraints.hasBoundedWidth) {
            Text(name, style = style, maxLines = maxLines, overflow = TextOverflow.Ellipsis)
            return@BoxWithConstraints
        }
        val width = constraints.maxWidth
        val shown = remember(name, width, style, maxLines, measurer) {
            AttachmentLogic.fitFileName(name) { candidate ->
                !measurer.measure(
                    text = candidate,
                    style = style,
                    overflow = TextOverflow.Clip,
                    softWrap = true,
                    maxLines = maxLines,
                    constraints = Constraints(maxWidth = width),
                ).hasVisualOverflow
            }
        }
        // Ellipsis only as a safety net: [shown] has been measured to fit.
        // The screen reader gets the whole name, not the cut one.
        Text(shown, style = style, maxLines = maxLines, overflow = TextOverflow.Ellipsis, modifier = Modifier.semantics { contentDescription = name })
    }
}

@Composable
private fun nameStyle(emphasized: Boolean): TextStyle =
    MaterialTheme.typography.labelLarge.copy(fontWeight = if (emphasized) FontWeight.SemiBold else FontWeight.Medium)

@Composable
private fun SizeText(attachment: Attachment) {
    Text(
        formatFileSize(LocalContext.current, attachment.size),
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        maxLines = 1,
        softWrap = false,
    )
}

// ---------------------------------------------------------------- tile

/** File tile: type badge, name (2 lines in the feed, whole in the detail), size below, and the action on the right. */
@Composable
private fun FileTile(
    attachment: Attachment,
    thumbnail: ImageBitmap?,
    action: AttachmentRowAction,
    external: Boolean,
    full: Boolean,
    emphasized: Boolean,
    onAltBackground: Boolean,
    onClick: (() -> Unit)?,
    onClickLabel: String?,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .clip(TomiTheme.shapes.thumbnail)
            .background(if (onAltBackground) MaterialTheme.colorScheme.surface else MaterialTheme.colorScheme.surfaceVariant)
            // Only a tile that does something reacts to a tap (ripple); the others pass it on to the card.
            .then(if (onClick != null) Modifier.clickable(onClickLabel = onClickLabel, onClick = onClick) else Modifier)
            // Reads as one element ("name, size"); the badge and the arrow are decoration.
            .semantics(mergeDescendants = true) {}
            .heightIn(min = TileMinHeight)
            .padding(8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        // Tonal = opens in Tomi; a file that goes to another app keeps the neutral badge.
        TypeBadge(
            attachment,
            thumbnail,
            tonal = action == AttachmentRowAction.OpenImage || action == AttachmentRowAction.OpenCodeFile,
            onAltBackground = onAltBackground,
        )
        Column(Modifier.weight(1f)) {
            FileName(attachment.name, nameStyle(emphasized), maxLines = if (full) null else 2)
            SizeText(attachment)
        }
        val trailing: ImageVector? = when {
            action == AttachmentRowAction.OpenExternal || external -> Icons.AutoMirrored.Outlined.OpenInNew
            action != AttachmentRowAction.None -> Icons.AutoMirrored.Outlined.KeyboardArrowRight
            else -> null
        }
        if (trailing != null) {
            Icon(
                trailing,
                contentDescription = null,
                modifier = Modifier.padding(end = 4.dp).size(20.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/**
 * Type badge 40 x 40: the mini-thumbnail of an image; else the upper-case extension (4 letters at most) or an icon. Tonal (primary
 * container, mono) when the file opens in Tomi, neutral (surface + border) for any other file.
 */
@Composable
private fun TypeBadge(attachment: Attachment, thumbnail: ImageBitmap?, tonal: Boolean, onAltBackground: Boolean) {
    val shape = MaterialTheme.shapes.extraSmall
    val colors = MaterialTheme.colorScheme
    val base = Modifier.size(BadgeSize).clip(shape).clearAndSetSemantics {}
    if (thumbnail != null) {
        Image(thumbnail, contentDescription = null, contentScale = ContentScale.Crop, modifier = base)
        return
    }
    val background = when {
        tonal -> colors.primaryContainer
        onAltBackground -> colors.surfaceVariant
        else -> colors.surface
    }
    val content = if (tonal) colors.primary else colors.onSurface
    // An image has no extension label: its icon (the name tells the rest).
    val extension = if (attachment.isImage) null else AttachmentLogic.badgeExtension(attachment.name)
    Box(
        base.background(background).then(if (tonal) Modifier else Modifier.border(BorderStroke(1.dp, colors.outlineVariant), shape)),
        contentAlignment = Alignment.Center,
    ) {
        if (extension != null) {
            Text(
                extension,
                style = MaterialTheme.typography.bodySmall.copy(
                    fontWeight = FontWeight.SemiBold,
                    letterSpacing = 0.5.sp,
                    fontFamily = if (tonal) TomiFonts.mono else TomiFonts.ui,
                    fontFeatureSettings = if (tonal) TomiFonts.NO_LIGATURES else null,
                ),
                color = content,
                maxLines = 1,
                softWrap = false,
            )
        } else {
            val icon = when {
                attachment.isImage -> Icons.Outlined.Image
                CodeFiles.isCodeFile(attachment) -> Icons.Outlined.Code
                else -> Icons.AutoMirrored.Outlined.InsertDriveFile
            }
            Icon(icon, contentDescription = null, modifier = Modifier.size(20.dp), tint = content)
        }
    }
}

// ---------------------------------------------------------------- image frame

/** Image: the thumbnail and its caption (name, size, chevron) in one frame, one touch target. */
@Composable
private fun ImageFrame(
    attachment: Attachment,
    bitmap: ImageBitmap,
    maxHeight: Dp?,
    contentScale: ContentScale,
    full: Boolean,
    emphasized: Boolean,
    hasAction: Boolean,
    onClick: (() -> Unit)?,
    onClickLabel: String?,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(TomiTheme.shapes.thumbnail)
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .then(if (onClick != null) Modifier.clickable(onClickLabel = onClickLabel, onClick = onClick) else Modifier)
            .semantics(mergeDescendants = true) {},
    ) {
        Image(
            bitmap = bitmap,
            // The caption below says the name; the frame is read as one element.
            contentDescription = null,
            contentScale = contentScale,
            modifier = Modifier
                .fillMaxWidth()
                .then(if (maxHeight != null) Modifier.heightIn(max = maxHeight) else Modifier),
        )
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 40.dp)
                .padding(start = 12.dp, top = 8.dp, end = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            FileName(attachment.name, nameStyle(emphasized), maxLines = if (full) null else 1, modifier = Modifier.weight(1f))
            SizeText(attachment)
            if (hasAction) {
                Icon(
                    Icons.AutoMirrored.Outlined.KeyboardArrowRight,
                    contentDescription = null,
                    modifier = Modifier.padding(end = 4.dp).size(20.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

// ---------------------------------------------------------------- mosaic

/**
 * Images only (2 or more): 2 square cells, or a large one on the left (2/3) and two small ones; "+N" on the last cell, which opens
 * the detail. Each other cell opens the image viewer at its index.
 */
@Composable
private fun Mosaic(
    attachments: List<Attachment>,
    bitmaps: List<ImageBitmap?>,
    plan: AttachmentLogic.Plan.Mosaic,
    onOpenImage: ((Int) -> Unit)?,
    onOpenDetail: (() -> Unit)?,
) {
    val viewLabel = stringResource(R.string.action_view_image)
    val detailLabel = stringResource(R.string.action_details)
    @Composable
    fun cell(index: Int, modifier: Modifier) {
        val last = index == plan.shown - 1
        val more = if (last) plan.more else 0
        val onClick: (() -> Unit)? = when {
            more > 0 && onOpenDetail != null -> onOpenDetail
            more == 0 && onOpenImage != null -> ({ onOpenImage(index) })
            else -> null
        }
        MosaicCell(
            attachment = attachments[index],
            bitmap = bitmaps[index],
            more = more,
            onClick = onClick,
            onClickLabel = if (more > 0) detailLabel else viewLabel,
            modifier = modifier,
        )
    }

    val gap = 4.dp
    if (plan.shown == 2) {
        Row(
            Modifier.fillMaxWidth().aspectRatio(2.2f).clip(TomiTheme.shapes.thumbnail),
            horizontalArrangement = Arrangement.spacedBy(gap),
        ) {
            cell(0, Modifier.weight(1f).fillMaxHeight())
            cell(1, Modifier.weight(1f).fillMaxHeight())
        }
    } else {
        Row(
            Modifier.fillMaxWidth().aspectRatio(5f / 3f).clip(TomiTheme.shapes.thumbnail),
            horizontalArrangement = Arrangement.spacedBy(gap),
        ) {
            cell(0, Modifier.weight(2f).fillMaxHeight())
            Column(Modifier.weight(1f).fillMaxHeight(), verticalArrangement = Arrangement.spacedBy(gap)) {
                cell(1, Modifier.weight(1f).fillMaxWidth())
                cell(2, Modifier.weight(1f).fillMaxWidth())
            }
        }
    }
}

@Composable
private fun MosaicCell(
    attachment: Attachment,
    bitmap: ImageBitmap?,
    more: Int,
    onClick: (() -> Unit)?,
    onClickLabel: String?,
    modifier: Modifier,
) {
    val moreText = if (more > 0) pluralStringResource(R.plurals.more_files, more, more) else null
    Box(
        modifier
            .then(if (onClick != null) Modifier.clickable(onClickLabel = onClickLabel, onClick = onClick) else Modifier)
            .then(if (moreText != null) Modifier.semantics { contentDescription = moreText } else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        if (bitmap != null) {
            Image(
                bitmap = bitmap,
                contentDescription = if (moreText == null) attachment.name else null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        } else {
            Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceVariant))
        }
        if (more > 0) {
            Box(Modifier.fillMaxSize().background(MosaicVeil).clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
                Text(
                    "+$more",
                    style = MaterialTheme.typography.titleLarge,
                    color = Color.White,
                )
            }
        }
    }
}
