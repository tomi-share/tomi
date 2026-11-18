package eu.shaffe.tomi.ui.viewer

import android.content.Context
import android.provider.Settings
import android.widget.Toast
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.DeleteForever
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.SaveAlt
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.ui.common.TomiSnackbarHost
import eu.shaffe.tomi.ui.common.FlagSecureEffect
import eu.shaffe.tomi.ui.common.findActivity
import eu.shaffe.tomi.ui.common.rememberAuthenticator
import eu.shaffe.tomi.ui.feed.FeedEventsEffect
import eu.shaffe.tomi.ui.feed.AttachmentRemovals
import eu.shaffe.tomi.ui.feed.FeedViewModel
import eu.shaffe.tomi.ui.theme.TomiTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** color.dark.bg of design/tomi/tokens.json: the viewer is always on the night background, whatever the app theme. */
private val ViewerBackground = Color(0xFF121620)
private val ViewerForeground = Color.White

/**
 * Internal full-screen image viewer, reached by a tap on an image (`viewer/{id}/{slot}`).
 *
 * - Immersive: night background, centred image, system bars hidden. A single tap shows / hides the top bar (back arrow) and
 *   the action bar at the bottom (save, share, details, delete) together with the system bars; hidden by default.
 * - EXIF orientation is applied when decoding ([ImageBitmapLoader]), so a portrait photo stored sideways is shown upright.
 * - Pinch to zoom, double tap to zoom in / out, drag to move when zoomed in ([ZoomMath]); a horizontal swipe pages between the
 *   images of the item (position "2/5" at the bottom).
 * - Actions (bottom bar, no "..." menu): save to the gallery, share, details, delete. They concern the image on screen, by its
 *   SLOT, never the other images of the item. "Supprimer" removes that image only (from this device, or everywhere when the relay
 *   supports it); the pager follows the list when an image disappears, and the viewer closes when no image is left. Deleting a
 *   whole item stays in the feed menu and the detail screen.
 * - Hidden ("Masqué") item: nothing is decrypted or drawn before the biometric gate passed, saving and sharing ask the gate
 *   again (same 2-minute window), the window is FLAG_SECURE, and the viewer closes when the app leaves the screen.
 */
@Composable
fun ImageViewerScreen(
    itemId: String,
    startSlot: Int,
    viewModel: FeedViewModel,
    onBack: () -> Unit,
    onOpenDetail: () -> Unit,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val item = state.allItems.firstOrNull { it.id == itemId }
    val snackbar = remember { SnackbarHostState() }
    FeedEventsEffect(viewModel, snackbar)
    val authenticator = rememberAuthenticator()
    val currentOnBack by rememberUpdatedState(onBack)

    val isSecret = item?.secret == true
    var unlocked by remember { mutableStateOf(false) }
    LaunchedEffect(item?.id, isSecret) {
        val current = item
        if (current == null || !current.secret) return@LaunchedEffect
        unlocked = false
        val allowed = viewModel.authorizeImageViewer(current) { authenticator?.forImageView() ?: AuthOutcome.Unavailable }
        if (allowed) unlocked = true else currentOnBack()
    }

    // Deleting the item from here closes the viewer; an id that never existed shows the empty message.
    var existed by remember { mutableStateOf(false) }
    LaunchedEffect(item) {
        if (item != null) existed = true else if (existed) currentOnBack()
    }
    // The capability of the relay decides whether "Supprimer l'image partout" is offered.
    LaunchedEffect(viewModel) { viewModel.refreshAttachmentRemovalSupport() }

    FlagSecureEffect(enabled = isSecret)
    // A hidden image does not stay on screen when the app leaves it (like the revealed secrets of the feed). A rotation also
    // stops the activity: the viewer then stays, and asks the gate again after the re-creation (instant inside the 2-minute window).
    val activity = LocalContext.current.findActivity()
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) {
        if (isSecret && activity?.isChangingConfigurations != true) currentOnBack()
    }

    val content = ViewerGate.content(item, loaded = state.loaded, unlocked = unlocked)
    // The last image of the item was removed (here, or on another device): nothing is left to look at, the viewer closes.
    var hadImages by remember { mutableStateOf(false) }
    LaunchedEffect(content) {
        if (content is ViewerContent.Pages) hadImages = true else if (content == ViewerContent.Empty && hadImages) currentOnBack()
    }
    Box(
        Modifier
            .fillMaxSize()
            .background(ViewerBackground),
    ) {
        when (content) {
            ViewerContent.Loading -> Unit
            ViewerContent.Locked -> MessageWithBack(R.string.viewer_locked, onBack)
            ViewerContent.Empty -> MessageWithBack(R.string.viewer_no_image, onBack)
            is ViewerContent.Pages -> if (item != null) {
                ViewerPages(
                    item = item,
                    images = content.images,
                    startSlot = startSlot,
                    viewModel = viewModel,
                    onBack = onBack,
                    onOpenDetail = onOpenDetail,
                )
            }
        }
        TomiSnackbarHost(
            snackbar,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding()
                .padding(bottom = 88.dp),
        )
    }
}

@Composable
private fun MessageWithBack(messageRes: Int, onBack: () -> Unit) {
    // The system bars stay visible here: there is nothing to look at, only a way out.
    Box(Modifier.fillMaxSize()) {
        Text(
            stringResource(messageRes),
            style = MaterialTheme.typography.bodyLarge,
            color = ViewerForeground,
            textAlign = TextAlign.Center,
            modifier = Modifier
                .align(Alignment.Center)
                .padding(32.dp),
        )
        ViewerTopBar(visible = true, onBack = onBack)
    }
}

@Composable
private fun ViewerPages(
    item: TomiItem,
    images: List<ViewerImage>,
    startSlot: Int,
    viewModel: FeedViewModel,
    onBack: () -> Unit,
    onOpenDetail: () -> Unit,
) {
    val context = LocalContext.current
    val authenticator = rememberAuthenticator()
    val pagerState = rememberPagerState(initialPage = ViewerImages.pageFor(images, startSlot)) { images.size }
    var barsVisible by rememberSaveable { mutableStateOf(false) }
    ImmersiveSystemBars(barsVisible)

    // The position counter fades out after ViewerCounter.IDLE_MS without interaction. A page change, a swipe or a tap
    // (a tap flips barsVisible) wakes it up and restarts the timer; it also stays on while the bars are shown.
    var recentlyActive by remember { mutableStateOf(true) }
    val scrolling = pagerState.isScrollInProgress
    LaunchedEffect(pagerState.currentPage, scrolling, barsVisible) {
        recentlyActive = true
        if (!scrolling) {
            delay(ViewerCounter.IDLE_MS)
            recentlyActive = false
        }
    }
    val reduceMotion = remember(context) { isReducedMotion(context) }

    val chooserTitle = stringResource(R.string.share_chooser_title)
    val noApp = stringResource(R.string.share_no_app)
    // The slot of the image to delete everywhere, asked to confirm: fixed when the menu was used, even if the pager moves meanwhile.
    var confirmDeleteEverywhereSlot by remember { mutableStateOf<Int?>(null) }
    val canDelete = AttachmentRemovals.canRemove(item)
    val relaySupportsRemoval by viewModel.attachmentRemovalEverywhere.collectAsStateWithLifecycle()
    // Null when the page is out of range (the list just changed): the action then does nothing instead of aiming at another image.
    fun currentSlot(): Int? = ViewerImages.slotAt(images, pagerState.currentPage)

    Box(Modifier.fillMaxSize()) {
        HorizontalPager(
            state = pagerState,
            pageSpacing = 16.dp,
            // The slot is the key: the pager keeps the same image on screen when another one disappears before it.
            key = { page -> images.getOrNull(page)?.slot ?: page },
            modifier = Modifier.fillMaxSize(),
        ) { page ->
            val description = stringResource(R.string.viewer_image_description, page + 1, images.size)
            ZoomableImagePage(
                itemId = item.id,
                slot = images[page].slot,
                description = description,
                viewModel = viewModel,
                onToggleBars = { barsVisible = !barsVisible },
            )
        }

        if (images.size > 1) {
            val position = stringResource(R.string.viewer_position, pagerState.currentPage + 1, images.size)
            // Reduce motion (animator scale 0): the counter appears and disappears at once, but still goes away when idle.
            AnimatedVisibility(
                visible = ViewerCounter.isVisible(barsVisible, recentlyActive),
                modifier = Modifier.align(Alignment.BottomCenter),
                enter = if (reduceMotion) EnterTransition.None else fadeIn(),
                exit = if (reduceMotion) ExitTransition.None else fadeOut(),
            ) {
                Surface(
                    color = Color.Black.copy(alpha = 0.55f),
                    contentColor = ViewerForeground,
                    shape = CircleShape,
                    modifier = Modifier
                        .navigationBarsPadding()
                        // Above the action bar while it is shown.
                        .padding(bottom = if (barsVisible) ViewerBarHeight + 8.dp else 16.dp)
                        .semantics { contentDescription = position },
                ) {
                    Text(
                        position,
                        style = MaterialTheme.typography.labelLarge,
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = 6.dp),
                    )
                }
            }
        }

        ViewerTopBar(visible = barsVisible, onBack = onBack)
        ViewerActionBar(
            visible = barsVisible,
            modifier = Modifier.align(Alignment.BottomCenter),
            // A hidden item is never written to the shared gallery (no permanent clear copy); sharing it stays possible.
            canSave = ViewerActions.canSaveToGallery(item),
            canDelete = canDelete,
            canDeleteEverywhere = relaySupportsRemoval,
            onSave = {
                currentSlot()?.let { slot ->
                    viewModel.saveImage(
                        item = item,
                        slot = slot,
                        authenticate = { authenticator?.forImageSave() ?: AuthOutcome.Unavailable },
                        write = { uri, attachment -> ImageExport.saveToGallery(context.applicationContext, uri, attachment) },
                    )
                }
            },
            onShare = {
                currentSlot()?.let { slot ->
                    viewModel.shareImage(
                        item = item,
                        slot = slot,
                        authenticate = { authenticator?.forShare() ?: AuthOutcome.Unavailable },
                        launch = { uri, attachment ->
                            // "No app can receive it" is told here; the action itself did what it could.
                            if (!ImageExport.share(context, uri, attachment, chooserTitle)) {
                                Toast.makeText(context, noApp, Toast.LENGTH_SHORT).show()
                            }
                            true
                        },
                    )
                }
            },
            onDetails = onOpenDetail,
            onDeleteLocal = { currentSlot()?.let { viewModel.deleteAttachment(item, it, everywhere = false) } },
            onDeleteEverywhere = { confirmDeleteEverywhereSlot = currentSlot() },
        )
    }

    confirmDeleteEverywhereSlot?.let { slot ->
        DeleteImageEverywhereDialog(
            secret = item.secret,
            onConfirm = {
                confirmDeleteEverywhereSlot = null
                viewModel.deleteAttachment(item, slot, everywhere = true)
            },
            onDismiss = { confirmDeleteEverywhereSlot = null },
        )
    }
}

/** True when the system animation scale is 0 (Settings > Accessibility > Remove animations): no animated fades then. */
private fun isReducedMotion(context: Context): Boolean =
    Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f

/** Height of the action bar, without the system navigation bar. */
private val ViewerBarHeight = 72.dp

/** Top bar over the image: the back arrow, on a dark scrim. Fades in and out with the system bars. */
@Composable
private fun ViewerTopBar(visible: Boolean, onBack: () -> Unit) {
    AnimatedVisibility(visible = visible, enter = fadeIn(), exit = fadeOut()) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.65f), Color.Transparent)))
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))
                .padding(horizontal = 4.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack, colors = IconButtonDefaults.iconButtonColors(contentColor = ViewerForeground)) {
                Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_back))
            }
        }
    }
}

/**
 * Action bar at the bottom of the image, on a dark scrim, shown with the top bar: save, share, details, delete. Every action
 * is in sight (no "..." menu). Delete only concerns the image on screen and offers the two scopes in a small menu ("everywhere" only
 * when the relay supports removing an attachment).
 */
@Composable
private fun ViewerActionBar(
    visible: Boolean,
    modifier: Modifier,
    canSave: Boolean,
    canDelete: Boolean,
    canDeleteEverywhere: Boolean,
    onSave: () -> Unit,
    onShare: () -> Unit,
    onDetails: () -> Unit,
    onDeleteLocal: () -> Unit,
    onDeleteEverywhere: () -> Unit,
) {
    var deleteMenu by remember { mutableStateOf(false) }
    AnimatedVisibility(visible = visible, modifier = modifier, enter = fadeIn(), exit = fadeOut()) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.65f))))
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Bottom + WindowInsetsSides.Horizontal))
                .height(ViewerBarHeight),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            if (canSave) ViewerAction(Icons.Outlined.SaveAlt, R.string.viewer_save, onSave)
            ViewerAction(Icons.Outlined.Share, R.string.action_share, onShare)
            ViewerAction(Icons.Outlined.Info, R.string.action_details, onDetails)
            if (canDelete) {
                Box {
                    ViewerAction(Icons.Outlined.DeleteOutline, R.string.viewer_action_delete) { deleteMenu = true }
                    DropdownMenu(
                        expanded = deleteMenu,
                        onDismissRequest = { deleteMenu = false },
                        shape = TomiTheme.shapes.menu,
                        containerColor = MaterialTheme.colorScheme.surface,
                    ) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.viewer_delete_local)) },
                            leadingIcon = { Icon(Icons.Outlined.DeleteOutline, contentDescription = null) },
                            onClick = {
                                deleteMenu = false
                                onDeleteLocal()
                            },
                        )
                        if (canDeleteEverywhere) {
                            DropdownMenuItem(
                                text = { Text(stringResource(R.string.viewer_delete_everywhere)) },
                                leadingIcon = { Icon(Icons.Outlined.DeleteForever, contentDescription = null) },
                                colors = MenuDefaults.itemColors(
                                    textColor = MaterialTheme.colorScheme.error,
                                    leadingIconColor = MaterialTheme.colorScheme.error,
                                ),
                                onClick = {
                                    deleteMenu = false
                                    onDeleteEverywhere()
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}

/** One action of the bar: icon over a short label, a 48 dp tall touch target. */
@Composable
private fun ViewerAction(icon: ImageVector, labelRes: Int, onClick: () -> Unit) {
    val label = stringResource(labelRes)
    Column(
        Modifier
            .clip(TomiTheme.shapes.button)
            .clickable(role = Role.Button, onClick = onClick)
            .defaultMinSize(minWidth = 64.dp, minHeight = 48.dp)
            .padding(horizontal = 8.dp, vertical = 4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(icon, contentDescription = null, tint = ViewerForeground)
        Text(label, style = MaterialTheme.typography.labelMedium, color = ViewerForeground, maxLines = 1)
    }
}

/** Confirmation of "delete this image everywhere": it names the image, not the item. */
@Composable
private fun DeleteImageEverywhereDialog(secret: Boolean, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        shape = TomiTheme.shapes.menu,
        title = { Text(stringResource(R.string.dialog_delete_image_everywhere_title)) },
        text = {
            Text(stringResource(if (secret) R.string.dialog_delete_image_everywhere_secret else R.string.dialog_delete_image_everywhere_message))
        },
        confirmButton = {
            TextButton(onClick = onConfirm) {
                Text(stringResource(R.string.viewer_delete_everywhere), color = MaterialTheme.colorScheme.error)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

/**
 * Hides the system bars while [barsVisible] is false (swiping from the edge shows them for a moment), shows them when the
 * bars are asked for, and puts everything back when the viewer is left. The bars are drawn light on the night background.
 */
@Composable
private fun ImmersiveSystemBars(barsVisible: Boolean) {
    val view = LocalView.current
    val window = view.context.findActivity()?.window
    DisposableEffect(window, view) {
        val controller = window?.let { WindowCompat.getInsetsController(it, view) }
        val lightStatus = controller?.isAppearanceLightStatusBars
        val lightNavigation = controller?.isAppearanceLightNavigationBars
        controller?.isAppearanceLightStatusBars = false
        controller?.isAppearanceLightNavigationBars = false
        controller?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        onDispose {
            if (controller != null) {
                controller.show(WindowInsetsCompat.Type.systemBars())
                lightStatus?.let { controller.isAppearanceLightStatusBars = it }
                lightNavigation?.let { controller.isAppearanceLightNavigationBars = it }
            }
        }
    }
    LaunchedEffect(barsVisible, window, view) {
        val controller = window?.let { WindowCompat.getInsetsController(it, view) } ?: return@LaunchedEffect
        if (barsVisible) controller.show(WindowInsetsCompat.Type.systemBars()) else controller.hide(WindowInsetsCompat.Type.systemBars())
    }
}

/** State of the decoding of one page. */
private sealed interface PageImage {
    data object Loading : PageImage
    data object Failed : PageImage
    class Ready(val bitmap: ImageBitmap) : PageImage
}

@Composable
private fun ZoomableImagePage(
    itemId: String,
    slot: Int,
    description: String,
    viewModel: FeedViewModel,
    onToggleBars: () -> Unit,
) {
    val resolver = LocalContext.current.contentResolver
    val page by produceState<PageImage>(PageImage.Loading, itemId, slot) {
        // The file is the decrypted copy in the core cache; it is read, never copied.
        val uri = viewModel.imageFile(itemId, slot)
        val bitmap = uri?.let { withContext(Dispatchers.IO) { ImageBitmapLoader.load(resolver, it) } }
        value = if (bitmap == null) PageImage.Failed else PageImage.Ready(bitmap.asImageBitmap())
    }
    when (val current = page) {
        PageImage.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            // Charter 6.3: a linear bar while the image is decoded, never a circular spinner.
            LinearProgressIndicator(modifier = Modifier.width(160.dp), color = ViewerForeground, trackColor = ViewerForeground.copy(alpha = 0.24f))
        }
        PageImage.Failed -> Box(
            Modifier
                .fillMaxSize()
                .pointerInput(Unit) { detectTapGestures(onTap = { onToggleBars() }) },
            contentAlignment = Alignment.Center,
        ) {
            Text(
                stringResource(R.string.viewer_load_failed),
                style = MaterialTheme.typography.bodyLarge,
                color = ViewerForeground,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(32.dp),
            )
        }
        is PageImage.Ready -> ZoomableImage(current.bitmap, description, onToggleBars)
    }
}

/**
 * The image, fitted in the view, with pinch zoom, double-tap zoom and drag. A one-finger drag at the fitted size is left to the
 * pager (page change); once zoomed in, the drag moves the image and is consumed so the pager stays still.
 */
@Composable
private fun ZoomableImage(bitmap: ImageBitmap, description: String, onToggleBars: () -> Unit) {
    var placement by remember(bitmap) { mutableStateOf(Placement()) }
    var viewSize by remember { mutableStateOf(IntSize.Zero) }
    val currentOnToggleBars by rememberUpdatedState(onToggleBars)

    Box(
        Modifier
            .fillMaxSize()
            .clipToBounds()
            .onSizeChanged { viewSize = it }
            .pointerInput(bitmap) {
                detectTapGestures(
                    onTap = { currentOnToggleBars() },
                    onDoubleTap = { tap ->
                        val viewW = viewSize.width.toFloat()
                        val viewH = viewSize.height.toFloat()
                        val (fitW, fitH) = ZoomMath.fitSize(bitmap.width.toFloat(), bitmap.height.toFloat(), viewW, viewH)
                        placement = ZoomMath.doubleTap(placement, tap.x - viewW / 2f, tap.y - viewH / 2f, fitW, fitH, viewW, viewH)
                    },
                )
            }
            .pointerInput(bitmap) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    do {
                        val event = awaitPointerEvent(PointerEventPass.Main)
                        val cancelled = event.changes.any { it.isConsumed }
                        if (!cancelled) {
                            val fingers = event.changes.count { it.pressed }
                            val zoom = event.calculateZoom()
                            val pan = event.calculatePan()
                            if ((fingers > 1 || ZoomMath.isZoomed(placement)) && (zoom != 1f || pan.x != 0f || pan.y != 0f)) {
                                val viewW = viewSize.width.toFloat()
                                val viewH = viewSize.height.toFloat()
                                val (fitW, fitH) = ZoomMath.fitSize(bitmap.width.toFloat(), bitmap.height.toFloat(), viewW, viewH)
                                val centroid = event.calculateCentroid(useCurrent = false)
                                placement = ZoomMath.transform(
                                    placement, zoom, pan.x, pan.y,
                                    centroid.x - viewW / 2f, centroid.y - viewH / 2f,
                                    fitW, fitH, viewW, viewH,
                                )
                                event.changes.forEach { if (it.positionChanged()) it.consume() }
                            }
                        }
                    } while (!cancelled && event.changes.any { it.pressed })
                }
            },
    ) {
        Image(
            bitmap = bitmap,
            contentDescription = description,
            contentScale = ContentScale.Fit,
            filterQuality = FilterQuality.High,
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer {
                    scaleX = placement.scale
                    scaleY = placement.scale
                    translationX = placement.x
                    translationY = placement.y
                },
        )
    }
}
