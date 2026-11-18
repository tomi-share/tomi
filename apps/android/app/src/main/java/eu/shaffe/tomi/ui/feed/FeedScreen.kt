package eu.shaffe.tomi.ui.feed

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.Notes
import androidx.compose.material.icons.automirrored.outlined.Send
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.CloudOff
import androidx.compose.material.icons.outlined.Devices
import androidx.compose.material.icons.automirrored.outlined.InsertDriveFile
import androidx.compose.material.icons.automirrored.outlined.Logout
import androidx.compose.material.icons.outlined.FilterList
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.Link
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PushPin
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.background
import androidx.compose.runtime.Stable
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.lazy.LazyListState
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import android.os.SystemClock
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.DeliveryState
import eu.shaffe.tomi.core.DepartureKind
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.isOutgoing
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.ui.common.FeedbackTone
import eu.shaffe.tomi.ui.common.TomiSnackbarHost
import eu.shaffe.tomi.send.Submission
import eu.shaffe.tomi.ui.send.SentBanner
import eu.shaffe.tomi.ui.common.showFeedback
import eu.shaffe.tomi.ui.common.TomiFloatingToolbar
import eu.shaffe.tomi.ui.common.measureAsToolbar
import eu.shaffe.tomi.ui.common.rememberToolbarClearance
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.common.ToolbarFab
import eu.shaffe.tomi.ui.common.ToolbarIconButton
import eu.shaffe.tomi.share.FileSharing
import eu.shaffe.tomi.ui.common.launchFileShare
import eu.shaffe.tomi.ui.common.launchSystemShare
import eu.shaffe.tomi.ui.common.openAttachmentUri
import eu.shaffe.tomi.ui.common.openHttpLink
import eu.shaffe.tomi.ui.common.rememberAuthenticator
import eu.shaffe.tomi.ui.render.PreviewKind
import eu.shaffe.tomi.ui.render.RenderModes
import eu.shaffe.tomi.ui.theme.TomiLogo
import eu.shaffe.tomi.ui.theme.TomiTheme
import eu.shaffe.tomi.ui.common.containerViewModel
import kotlinx.coroutines.launch

/** Gap between two cards in the feed (unchanged). */
private val CardSpacing = 12.dp

/** Extra space above a section title: [CardSpacing] (item spacing) + this = 24 dp. */
private val SectionTopGap = 12.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FeedScreen(
    viewModel: FeedViewModel,
    onSend: () -> Unit,
    onDevices: () -> Unit,
    onShareHelp: () -> Unit,
    onOpenItem: (String) -> Unit,
    onOpenImage: (itemId: String, slot: Int) -> Unit,
    onOpenCodeFile: (itemId: String, slot: Int) -> Unit,
    /** The send that just left the compose screen, still within its undo window (null otherwise). */
    sentSubmission: Submission? = null,
    /** "Undo" / "Options" of that banner: takes the item back into the compose screen. */
    onUndoSend: () -> Unit = {},
    onSentBannerTimeout: () -> Unit = {},
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    FeedEventsEffect(viewModel, snackbar)
    // Scoped to this destination; the dialogs and the banner of the departure live in it (they are not part of the feed state).
    val leaveViewModel = containerViewModel { LeaveSpaceViewModel(it.core, it.departureRetrier) }
    val departureBanner by leaveViewModel.banner.collectAsStateWithLifecycle()
    val canOfferLeave by leaveViewModel.canOfferLeave.collectAsStateWithLifecycle()
    val inDemo by viewModel.isDemo.collectAsStateWithLifecycle()
    DepartureEventsEffect(leaveViewModel, snackbar)
    val onRetryDeparture = rememberDepartureRetry(leaveViewModel)

    var searching by rememberSaveable { mutableStateOf(false) }
    var filterMenuOpen by remember { mutableStateOf(false) }
    val clearance = rememberToolbarClearance()
    val filterActive = state.filter != FeedFilter.All
    // The filter menu cannot stay open under (or reappear after) the search: its button is gone while searching.
    LaunchedEffect(searching) { if (searching) filterMenuOpen = false }
    // The way out of the search: same for the arrow of the search bar, the toolbar button and the system "back". Clears the
    // query, so the feed is back to its unfiltered list; without it "back" would leave the app.
    val closeSearch = {
        viewModel.setQuery("")
        searching = false
    }
    BackHandler(enabled = searching, onBack = closeSearch)
    // Outside the search, an active filter (the chip of the header) is one more level to leave: "back" returns to the full list
    // before it leaves the app. While searching, the first "back" closes the search and this one takes over on the next.
    BackHandler(enabled = filterActive && !searching) { viewModel.setFilter(FeedFilter.All) }
    // Material "enterAlways" top bar for the logo header, see below. Shown again whenever the list it floats over changes.
    val quickReturn = remember { QuickReturnHeader() }
    val density = LocalDensity.current
    LaunchedEffect(searching, state.filter, state.query) { quickReturn.reset() }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
    ) { padding ->
        Box(
            Modifier
                .screenContentPadding(padding)
                .fillMaxSize()
                .nestedScroll(quickReturn.connection),
        ) {
            Column(Modifier.fillMaxSize()) {
                if (searching) {
                    SearchHeader(
                        query = state.query,
                        onQuery = viewModel::setQuery,
                        onClose = closeSearch,
                    )
                    FilterChips(state.filter, viewModel::setFilter)
                    if (FeedSearchDisplay.showsCounter(state.query, state.filter)) {
                        val count = pluralStringResource(R.plurals.search_results, state.items.size, state.items.size)
                        Text(
                            if (FeedSearchDisplay.showsMaskedNote(state.query, state.items)) {
                                stringResource(R.string.search_summary_with_note, count, stringResource(R.string.search_masked_note))
                            } else {
                                count
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                        )
                    }
                }

                val logoHeader: @Composable (Modifier) -> Unit = { modifier ->
                    FeedHeader(
                        offline = !state.isOnline && state.allItems.any { it.hasQueuedDelivery(state.localDeviceId) },
                        filter = state.filter,
                        onClearFilter = { viewModel.setFilter(FeedFilter.All) },
                        onShareHelp = onShareHelp,
                        onLeave = leaveViewModel::open.takeIf { canOfferLeave },
                        modifier = modifier,
                    )
                }
                val banner: @Composable () -> Unit = {
                    if (inDemo) DemoBanner(onLeave = viewModel::leaveDemo)
                    DepartureBanner(
                        state = departureBanner,
                        onRetry = onRetryDeparture,
                        onCancel = leaveViewModel::cancelFromBanner,
                        onEraseAnyway = leaveViewModel::openErase,
                        onLeave = leaveViewModel::open,
                    )
                }
                val empty = state.items.isEmpty() && state.loaded

                if (searching || empty) {
                    // The search field, or nothing to scroll: everything stays put above the results / the empty state.
                    if (!searching) logoHeader(Modifier)
                    banner()
                    if (empty) {
                        EmptyState(
                            filtered = state.query.isNotBlank() || filterActive,
                            archives = state.filter == FeedFilter.Archives && state.query.isBlank(),
                        )
                    } else {
                        FeedList(state, viewModel, clearance.bottomPadding, 0.dp, onOpenItem, onOpenImage, onOpenCodeFile)
                    }
                } else {
                    // The header floats over the list, which starts below it (top padding): it slides away as the feed
                    // scrolls down and comes back as soon as it scrolls up. The banner of a departure stays in view
                    // (spec/api-v1.md 7.9.2): only the logo row hides, the banner slides up to the top and stays there.
                    Box(Modifier.weight(1f).fillMaxWidth().clipToBounds()) {
                        FeedList(
                            state, viewModel, clearance.bottomPadding,
                            with(density) { quickReturn.blockHeight.toDp() },
                            onOpenItem, onOpenImage, onOpenCodeFile,
                        )
                        Column(
                            Modifier
                                .fillMaxWidth()
                                .onSizeChanged { quickReturn.blockHeight = it.height }
                                .graphicsLayer { translationY = quickReturn.offset }
                                .background(MaterialTheme.colorScheme.background),
                        ) {
                            logoHeader(Modifier.onSizeChanged { quickReturn.range = it.height.toFloat() })
                            banner()
                        }
                    }
                }
            }

            Box(
                Modifier
                    .align(Alignment.BottomCenter)
                    .navigationBarsPadding()
                    .padding(bottom = 16.dp),
            ) {
                TomiFloatingToolbar(
                    modifier = Modifier.measureAsToolbar(clearance),
                    fab = {
                        ToolbarFab(
                            contentDescription = stringResource(R.string.action_send),
                            icon = Icons.AutoMirrored.Outlined.Send,
                            onClick = onSend,
                        )
                    },
                ) {
                    ToolbarIconButton(
                        icon = Icons.Outlined.Search,
                        contentDescription = stringResource(R.string.action_search),
                        selected = searching,
                        onClick = {
                            if (searching) viewModel.setQuery("")
                            searching = !searching
                        },
                    )
                    // While searching, the chips under the field do the filtering: no filter button (the filter stays active).
                    if (!searching) {
                        Box {
                            ToolbarIconButton(
                                icon = Icons.Outlined.FilterList,
                                contentDescription = stringResource(R.string.action_filters),
                                selected = filterActive,
                                badge = filterActive,
                                onClick = { filterMenuOpen = true },
                            )
                            FilterMenu(
                                expanded = filterMenuOpen,
                                selected = state.filter,
                                onSelect = {
                                    viewModel.setFilter(it)
                                    filterMenuOpen = false
                                },
                                onDismiss = { filterMenuOpen = false },
                            )
                        }
                    }
                    ToolbarIconButton(
                        icon = Icons.Outlined.Devices,
                        contentDescription = stringResource(R.string.devices_title),
                        onClick = onDevices,
                    )
                }
            }

            sentSubmission?.let { sent ->
                SentBanner(
                    skippedFiles = sent.skippedFiles,
                    deadline = sent.deadline,
                    onUndo = onUndoSend,
                    onOptions = onUndoSend,
                    onTimeout = onSentBannerTimeout,
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .navigationBarsPadding()
                        .padding(start = 16.dp, end = 16.dp, bottom = 96.dp),
                )
            }

            TomiSnackbarHost(
                snackbar,
                modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 96.dp),
            )
        }
    }

    LeaveSpaceDialog(leaveViewModel)
}

/** The one-line confirmations of the departure ("Déconnexion annulée"...), in the feedback of the feed. */
@Composable
private fun DepartureEventsEffect(viewModel: LeaveSpaceViewModel, snackbar: SnackbarHostState) {
    val resources = LocalResources.current
    LaunchedEffect(viewModel) {
        viewModel.events.collect { event ->
            val message = when (event) {
                is LeaveEvent.Cancelled -> resources.getString(
                    if (event.kind == DepartureKind.Leave) R.string.departure_cancelled_leave else R.string.departure_cancelled_delete,
                )
                LeaveEvent.Failed -> resources.getString(R.string.departure_failed)
                LeaveEvent.BiometricUnavailable -> resources.getString(R.string.delete_space_unavailable_biometric)
            }
            launch { snackbar.showFeedback(message) }
        }
    }
}

private fun TomiItem.hasQueuedDelivery(localDeviceId: String): Boolean =
    isOutgoing(localDeviceId) && deliveries.any { it.state == DeliveryState.Queued }

private const val SECTION_HEADER_TYPE = -1

/**
 * What the card of the item is made of. The list keeps the composition of a card that left the screen for the next card of
 * the SAME type: a Markdown card then reuses a Markdown card (its native view included) instead of being rebuilt out of, say,
 * a hidden one. [swipeable]: the cards of "Recent" are wrapped in the swipe to archive, the pinned ones are not.
 */
private fun TomiItem.cardContentType(swipeable: Boolean): Int {
    val body = when {
        !isReadable -> 0
        secret -> 1
        else -> when (text?.let(RenderModes::previewKind)) {
            PreviewKind.Markdown -> 2
            PreviewKind.Html -> 3
            PreviewKind.Plain -> 4
            null -> 5
        }
    }
    return if (swipeable) body else body + 10
}

@Composable
private fun FeedList(
    state: FeedUiState,
    viewModel: FeedViewModel,
    bottomPadding: Dp,
    /** Space kept free at the top for a header floating over the list. */
    topPadding: Dp,
    onOpenItem: (String) -> Unit,
    onOpenImage: (itemId: String, slot: Int) -> Unit,
    onOpenCodeFile: (itemId: String, slot: Int) -> Unit,
) {
    val listState = rememberLazyListState()
    // The single card that is open (swipe to reveal), by item id. A plain `remember`: never restored, nothing open after a recreation.
    var openId by remember { mutableStateOf<String?>(null) }
    // Scrolling the list closes the open card (the card closes itself when it is no longer the open one).
    LaunchedEffect(listState) {
        snapshotFlow { listState.isScrollInProgress }.collect { scrolling -> if (scrolling) openId = null }
    }
    ReadWhenSeenEffect(listState, state.sections.pinned + state.sections.recent, viewModel)
    // While searching the results are one flat list (no section titles): pinned ones first, then the others.
    val flat = FeedSearchDisplay.isFlat(state.query)
    LazyColumn(
        state = listState,
        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = topPadding + 4.dp, bottom = bottomPadding),
        verticalArrangement = Arrangement.spacedBy(CardSpacing),
        modifier = Modifier.fillMaxSize(),
    ) {
        if (state.sections.pinned.isNotEmpty()) {
            if (!flat) {
                item(key = "header-pinned", contentType = SECTION_HEADER_TYPE) {
                    SectionHeader(stringResource(R.string.section_pinned), state.sections.pinned.size, pinned = true)
                }
            }
            items(state.sections.pinned, key = { it.id }, contentType = { it.cardContentType(swipeable = false) }) { item ->
                ItemCardHost(
                    item, state, viewModel, expanded = false,
                    onOpenDetail = { onOpenItem(item.id) },
                    onOpenImage = { index -> onOpenImage(item.id, index) },
                    onOpenCodeFile = { index -> onOpenCodeFile(item.id, index) },
                )
            }
        }
        if (state.sections.recent.isNotEmpty()) {
            if (!flat) {
                item(key = "header-recent", contentType = SECTION_HEADER_TYPE) {
                    val title = if (state.filter == FeedFilter.Archives) R.string.section_archives else R.string.section_recent
                    SectionHeader(stringResource(title), count = null, pinned = false)
                }
            }
            items(state.sections.recent, key = { it.id }, contentType = { it.cardContentType(swipeable = true) }) { item ->
                // Swipe to the left = archive (un-archive on an archived card). Not on the pinned cards above.
                SwipeToArchive(
                    action = ArchiveSwipe.swipeAction(item, state.localDeviceId),
                    onAction = { action -> viewModel.applyArchiveAction(item, action) },
                    // One open card at a time: opening this one makes `openId` change, which closes the previous one.
                    keepOpen = openId == item.id,
                    onOpenChange = { open ->
                        if (open) openId = item.id else if (openId == item.id) openId = null
                    },
                    modifier = Modifier.animateItem(),
                ) {
                    ItemCardHost(
                        item, state, viewModel, expanded = false,
                        onOpenDetail = { onOpenItem(item.id) },
                        onOpenImage = { index -> onOpenImage(item.id, index) },
                        onOpenCodeFile = { index -> onOpenCodeFile(item.id, index) },
                    )
                }
            }
        }
    }
}

/**
 * Reads the cards the user looks at: a card on screen (see [SeenTracker.visibleKeys]) for [SeenTracker.DWELL_MS] is marked
 * read. Only while this screen is resumed: in the background, or with another screen on top (the feed's back stack entry is
 * then no longer resumed), nothing is read and the time spent there does not count.
 */
@Composable
private fun ReadWhenSeenEffect(listState: LazyListState, items: List<TomiItem>, viewModel: FeedViewModel) {
    val currentItems by rememberUpdatedState(items)
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(listState, lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            val tracker = SeenTracker()
            fun now() = SystemClock.elapsedRealtime()
            snapshotFlow { SeenTracker.visibleKeys(listState.layoutInfo) }.collectLatest { onScreen ->
                tracker.update(onScreen, now())
                while (true) {
                    delay(tracker.nextDelay(now()) ?: break)
                    val due = tracker.takeDue(now()).toSet()
                    if (due.isNotEmpty()) viewModel.onItemsSeen(currentItems.filter { it.id in due })
                }
            }
        }
    }
}

/**
 * Material "enterAlways" behaviour for the header floating over the feed: it follows what the list really scrolled (post-scroll,
 * so a list too short to scroll never hides it), down to [range] (the height of the logo row) out of view.
 */
@Stable
private class QuickReturnHeader {
    /** Translation of the header block, from -[range] (logo row hidden) to 0 (shown). */
    var offset by mutableFloatStateOf(0f)
        private set

    /** Height in px of the part that hides (the logo row). */
    var range by mutableFloatStateOf(0f)

    /** Height in px of the whole floating block (logo row + banner): the list keeps it free at its top. */
    var blockHeight by mutableIntStateOf(0)

    fun reset() {
        offset = 0f
    }

    val connection = object : NestedScrollConnection {
        override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset {
            offset = (offset + consumed.y).coerceIn(-range, 0f)
            return Offset.Zero
        }
    }
}

/**
 * Section title ("Épinglés", "Récents"). The list spaces its items by [CardSpacing]; the title adds [SectionTopGap] on
 * top of that, so there are 24 dp (space.5) above a title and still [CardSpacing] between two cards and between the
 * title and its first card.
 */
@Composable
private fun SectionHeader(title: String, count: Int?, pinned: Boolean) {
    Row(
        Modifier.fillMaxWidth().padding(top = SectionTopGap),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (pinned) {
            Icon(Icons.Outlined.PushPin, contentDescription = null, modifier = Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
        if (count != null) {
            Text(count.toString(), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** Tidy header: the logotype alone, then (right) the offline chip or the active filter, and the "..." menu. */
@Composable
private fun FeedHeader(
    offline: Boolean,
    filter: FeedFilter,
    onClearFilter: () -> Unit,
    onShareHelp: () -> Unit,
    /** Null while a departure is pending or running: the banner of the feed handles it. */
    onLeave: (() -> Unit)?,
    modifier: Modifier = Modifier,
) {
    var menuOpen by remember { mutableStateOf(false) }
    Row(
        modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        TomiLogo(height = 32.dp, tint = MaterialTheme.colorScheme.onBackground)
        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
            if (offline) {
                StatusChip(
                    text = stringResource(R.string.banner_offline),
                    icon = Icons.Outlined.CloudOff,
                    tint = TomiTheme.colors.warning,
                    onClick = null,
                )
            } else if (filter != FeedFilter.All) {
                StatusChip(
                    text = stringResource(filter.labelRes()),
                    icon = filter.icon(),
                    tint = MaterialTheme.colorScheme.primary,
                    onClick = onClearFilter,
                    trailingIcon = Icons.Outlined.Close,
                    trailingDescription = stringResource(R.string.action_clear_filter),
                )
            }
        }
        Box {
            IconButton(onClick = { menuOpen = true }) {
                Icon(Icons.Outlined.MoreVert, contentDescription = stringResource(R.string.action_more))
            }
            DropdownMenu(
                expanded = menuOpen,
                onDismissRequest = { menuOpen = false },
                shape = TomiTheme.shapes.menu,
                containerColor = MaterialTheme.colorScheme.surface,
            ) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.menu_share_help)) },
                    onClick = {
                        menuOpen = false
                        onShareHelp()
                    },
                )
                if (onLeave != null) {
                    HorizontalDivider()
                    // The destructive entry, in the error colour (text and icon).
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.menu_leave_space)) },
                        leadingIcon = { Icon(Icons.AutoMirrored.Outlined.Logout, contentDescription = null) },
                        colors = MenuDefaults.itemColors(
                            textColor = MaterialTheme.colorScheme.error,
                            leadingIconColor = MaterialTheme.colorScheme.error,
                        ),
                        onClick = {
                            menuOpen = false
                            onLeave()
                        },
                    )
                }
            }
        }
    }
}

@Composable
private fun StatusChip(
    text: String,
    icon: ImageVector,
    tint: androidx.compose.ui.graphics.Color,
    onClick: (() -> Unit)?,
    trailingIcon: ImageVector? = null,
    trailingDescription: String? = null,
) {
    val modifier = if (onClick != null) {
        Modifier.clickableChip(onClick)
    } else {
        Modifier
    }
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.surface,
        contentColor = tint,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = modifier,
    ) {
        Row(
            Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(18.dp))
            Text(text, style = MaterialTheme.typography.labelLarge, maxLines = 1)
            if (trailingIcon != null) {
                Icon(trailingIcon, contentDescription = trailingDescription, modifier = Modifier.size(18.dp))
            }
        }
    }
}

private fun Modifier.clickableChip(onClick: () -> Unit): Modifier =
    this.then(Modifier.clip(CircleShape).clickable(onClick = onClick))

@Composable
private fun SearchHeader(query: String, onQuery: (String) -> Unit, onClose: () -> Unit) {
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    LaunchedEffect(Unit) { focus.requestFocus() }
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.surfaceVariant,
        // Explicit: the back arrow has the colour of the one of the top bars (onSurface), not the muted onSurfaceVariant.
        contentColor = MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onClose) {
                Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_close_search))
            }
            TextField(
                value = query,
                onValueChange = onQuery,
                singleLine = true,
                placeholder = { Text(stringResource(R.string.search_placeholder)) },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                // The results are already live: "Search" only closes the keyboard, the query stays.
                keyboardActions = KeyboardActions(onSearch = { keyboard?.hide() }),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = androidx.compose.ui.graphics.Color.Transparent,
                    unfocusedContainerColor = androidx.compose.ui.graphics.Color.Transparent,
                    focusedIndicatorColor = androidx.compose.ui.graphics.Color.Transparent,
                    unfocusedIndicatorColor = androidx.compose.ui.graphics.Color.Transparent,
                ),
                modifier = Modifier.weight(1f).focusRequester(focus),
            )
            if (query.isNotEmpty()) {
                IconButton(onClick = { onQuery("") }) {
                    Icon(Icons.Outlined.Close, contentDescription = stringResource(R.string.action_clear_filter))
                }
            }
        }
    }
}

@Composable
private fun FilterChips(selected: FeedFilter, onSelect: (FeedFilter) -> Unit) {
    LazyRow(
        contentPadding = PaddingValues(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(FeedFilter.entries) { filter ->
            FilterChip(
                selected = filter == selected,
                onClick = { onSelect(filter) },
                label = { Text(stringResource(filter.labelRes())) },
                leadingIcon = if (filter == selected) {
                    { Icon(Icons.Outlined.Check, contentDescription = null, modifier = Modifier.size(18.dp)) }
                } else {
                    null
                },
                shape = CircleShape,
            )
        }
    }
}

@Composable
private fun FilterMenu(expanded: Boolean, selected: FeedFilter, onSelect: (FeedFilter) -> Unit, onDismiss: () -> Unit) {
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        shape = TomiTheme.shapes.menu,
        containerColor = MaterialTheme.colorScheme.surface,
    ) {
        FeedFilter.entries.forEach { filter ->
            DropdownMenuItem(
                text = { Text(stringResource(filter.labelRes())) },
                leadingIcon = { Icon(filter.icon(), contentDescription = null) },
                trailingIcon = if (filter == selected) {
                    { Icon(Icons.Outlined.Check, contentDescription = null) }
                } else {
                    null
                },
                onClick = { onSelect(filter) },
            )
        }
    }
}

private fun FeedFilter.labelRes(): Int = when (this) {
    FeedFilter.All -> R.string.filter_all
    FeedFilter.Links -> R.string.filter_links
    FeedFilter.Text -> R.string.filter_text
    FeedFilter.Files -> R.string.filter_files
    FeedFilter.Secrets -> R.string.filter_secrets
    FeedFilter.Archives -> R.string.filter_archives
}

private fun FeedFilter.icon(): ImageVector = when (this) {
    FeedFilter.All -> Icons.Outlined.Inbox
    FeedFilter.Links -> Icons.Outlined.Link
    FeedFilter.Text -> Icons.AutoMirrored.Outlined.Notes
    FeedFilter.Files -> Icons.AutoMirrored.Outlined.InsertDriveFile
    FeedFilter.Secrets -> Icons.Outlined.VisibilityOff
    FeedFilter.Archives -> Icons.Outlined.Archive
}

@Composable
private fun EmptyState(filtered: Boolean, archives: Boolean) {
    Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(
            stringResource(
                when {
                    archives -> R.string.feed_empty_archives
                    filtered -> R.string.feed_empty_filtered
                    else -> R.string.feed_empty
                },
            ),
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** Builds the actions of one item: authentication for hidden items, links, pin, system share, deletion. */
@Composable
fun rememberCardActions(
    item: TomiItem,
    viewModel: FeedViewModel,
    onOpenDetail: () -> Unit,
    onOpenImage: (slot: Int) -> Unit = {},
    onOpenCodeFile: (slot: Int) -> Unit = {},
): CardActions {
    val context = LocalContext.current
    val authenticator = rememberAuthenticator()
    val chooserTitle = stringResource(R.string.share_chooser_title)
    val hiddenPreviewTitle = stringResource(R.string.share_hidden_preview_title)
    val noApp = stringResource(R.string.share_no_app)
    val currentOnOpenDetail by rememberUpdatedState(onOpenDetail)
    val currentOnOpenImage by rememberUpdatedState(onOpenImage)
    val currentOnOpenCodeFile by rememberUpdatedState(onOpenCodeFile)
    val removalEverywhere by viewModel.attachmentRemovalEverywhere.collectAsStateWithLifecycle()
    return remember(item, viewModel, authenticator, context, removalEverywhere) {
        val authenticate: suspend () -> AuthOutcome = { authenticator?.forSecret() ?: AuthOutcome.Unavailable }
        // Same gate as "Afficher" (same BiometricGate and 2-minute window), with its own prompt wording.
        val authenticateForShare: suspend () -> AuthOutcome = { authenticator?.forShare() ?: AuthOutcome.Unavailable }
        CardActions(
            onCardOpened = { viewModel.onCardOpened(item) },
            onOpenDetail = { currentOnOpenDetail() },
            onOpenAttachment = {
                // Single attachment: system viewer. Not available (not downloaded, no viewer app): the detail instead.
                viewModel.openAttachment(
                    item,
                    launch = { uri, mime -> openAttachmentUri(context, uri, mime) },
                    onUnavailable = { currentOnOpenDetail() },
                )
            },
            // Internal viewer (route "viewer/{id}/{slot}"): the gate for a hidden item is asked by the viewer itself.
            onOpenImage = { index -> currentOnOpenImage(index) },
            // Code screen (route "code/{id}/{slot}"): the gate for a hidden item is asked by the screen itself.
            onOpenCodeFile = { index -> currentOnOpenCodeFile(index) },
            onReveal = { viewModel.reveal(item, authenticate) },
            onHide = { viewModel.hide(item.id) },
            onCopy = { mode -> viewModel.copy(item, mode, authenticate) },
            onCopyLink = { viewModel.copyLink(item) },
            onOpenLink = {
                item.url?.let { if (openHttpLink(context, it)) viewModel.onOpened(item) }
            },
            onShare = {
                if (FileSharing.canShare(item)) {
                    // File / image item: the files go to the share sheet (after the gate for a hidden item).
                    viewModel.shareFiles(item, authenticateForShare) { files ->
                        val opened = launchFileShare(context, files, chooserTitle)
                        if (!opened) android.widget.Toast.makeText(context, noApp, android.widget.Toast.LENGTH_SHORT).show()
                        opened
                    }
                } else viewModel.share(item, authenticateForShare) { payload ->
                    val opened = launchSystemShare(context, payload, chooserTitle, hiddenPreviewTitle)
                    if (!opened) android.widget.Toast.makeText(context, noApp, android.widget.Toast.LENGTH_SHORT).show()
                    opened
                }
            },
            onTogglePin = { viewModel.togglePin(item) },
            onToggleArchive = { viewModel.applyArchiveAction(item) },
            onDeleteLocal = { viewModel.deleteLocal(item.id) },
            onDeleteEverywhere = { viewModel.deleteEverywhere(item.id) },
            onDeleteAttachment = { slot, everywhere -> viewModel.deleteAttachment(item, slot, everywhere) },
            attachmentRemovalEverywhere = removalEverywhere,
            onRenderMode = viewModel::setRenderMode,
        )
    }
}

/** Wires one card to the view model. */
@Composable
fun ItemCardHost(
    item: TomiItem,
    state: FeedUiState,
    viewModel: FeedViewModel,
    expanded: Boolean,
    onOpenDetail: () -> Unit,
    onOpenImage: (slot: Int) -> Unit = {},
    onOpenCodeFile: (slot: Int) -> Unit = {},
    showMenu: Boolean = true,
) {
    ItemCard(
        item = item,
        localDeviceId = state.localDeviceId,
        isOnline = state.isOnline,
        revealed = item.id in state.revealedIds,
        preferredMode = state.preferredRenderMode,
        expanded = expanded,
        showMenu = showMenu,
        actions = rememberCardActions(item, viewModel, onOpenDetail, onOpenImage, onOpenCodeFile),
    )
}

/** Shows the one-line confirmations (copied, deleted, biometrics missing) as snackbars. */
@Composable
fun FeedEventsEffect(viewModel: FeedViewModel, snackbar: SnackbarHostState) {
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    LaunchedEffect(viewModel) {
        viewModel.events.collect { event ->
            val undoLabel = resources.getString(R.string.action_undo)
            val message = resources.getString(
                when (event) {
                    // An ordinary copy: Android 13+ already confirms it itself (see ui/common/Feedback.kt), no second one from the app.
                    FeedEvent.Copied -> return@collect
                    FeedEvent.CopiedSecret -> R.string.snackbar_copied_secret
                    FeedEvent.BiometricUnavailable -> R.string.toast_biometric_unavailable
                    FeedEvent.DeletedLocal -> R.string.snackbar_deleted_local
                    FeedEvent.DeletedEverywhere -> R.string.snackbar_deleted_everywhere
                    FeedEvent.PinFailed -> R.string.snackbar_pin_failed
                    FeedEvent.ActionFailed -> R.string.snackbar_action_failed
                    is FeedEvent.ArchiveDone ->
                        if (event.action == ArchiveAction.Archive) R.string.snackbar_archived else R.string.snackbar_unarchived
                    FeedEvent.ArchiveFailed -> R.string.snackbar_archive_failed
                    FeedEvent.NothingToShare -> R.string.snackbar_nothing_to_share
                    FeedEvent.ImageSaved -> R.string.snackbar_image_saved
                    FeedEvent.ImageUnavailable -> R.string.snackbar_image_unavailable
                    FeedEvent.ImageActionFailed -> R.string.snackbar_image_failed
                    is FeedEvent.AttachmentDeleted -> when {
                        event.image && event.everywhere -> R.string.snackbar_image_deleted_everywhere
                        event.image -> R.string.snackbar_image_deleted_local
                        event.everywhere -> R.string.snackbar_file_deleted_everywhere
                        else -> R.string.snackbar_file_deleted_local
                    }
                },
            )
            scope.launch {
                if (event is FeedEvent.ArchiveDone) {
                    // "Archived · Undo" / "Unarchived · Undo": same banner as the other confirmations, with its action.
                    if (snackbar.showFeedback(message, undoLabel, FeedbackTone.Done) == SnackbarResult.ActionPerformed) viewModel.undoArchive(event)
                } else {
                    val done = event == FeedEvent.CopiedSecret || event == FeedEvent.ImageSaved ||
                        event == FeedEvent.DeletedLocal || event == FeedEvent.DeletedEverywhere || event is FeedEvent.AttachmentDeleted
                    snackbar.showFeedback(message, tone = if (done) FeedbackTone.Done else FeedbackTone.Neutral)
                }
            }
        }
    }
}
