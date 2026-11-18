package eu.shaffe.tomi.ui.code

import android.content.ContentResolver
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.outlined.KeyboardArrowUp
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.net.toUri
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
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import eu.shaffe.tomi.ui.feed.FeedEventsEffect
import eu.shaffe.tomi.ui.feed.FeedViewModel
import eu.shaffe.tomi.ui.theme.CodePalette
import eu.shaffe.tomi.ui.theme.TomiFonts
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.IOException

/**
 * Dedicated full-screen reader for code files (`code/{id}/{index}`), reached from a tap on a code file of a card or of the detail.
 *
 * - The file is read from the core's decrypted cache (`TomiCore.attachmentUri`), into memory only (2 MB at most): no copy of the
 *   clear text is ever written by the app.
 * - Coloured by tree-sitter (Enriched Markdown grammars) with the charter palette; monospace; vertical scroll, and horizontal scroll unless
 *   the line wrap is on ("Retour à la ligne" in the ⋮ menu, off by default, one persisted choice for every file). A language
 *   without grammar, or a file over the library limits (50 KB / 2,000 lines), is shown as plain monospace text.
 * - Search in the file: occurrences highlighted (ochre), the current one more pressed, previous / next, "n/m" counter, scroll to it.
 * - Several code files in the item: a tab bar on top and the "2/3" position in the app bar. No pager: it would fight the horizontal
 *   scroll of the code.
 * - Hidden ("Masqué") item: nothing is read or decoded before the biometric gate passed (same gate and 2-minute window as "Afficher"),
 *   the window is FLAG_SECURE, the screen closes when the app leaves the screen, and "Copier" goes through the sensitive clipboard.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CodeFileScreen(
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
        val allowed = viewModel.authorizeCodeScreen(current) { authenticator?.forFileView() ?: AuthOutcome.Unavailable }
        if (allowed) unlocked = true else currentOnBack()
    }

    // Deleting the item from here closes the screen; an id that never existed shows the empty message.
    var existed by remember { mutableStateOf(false) }
    LaunchedEffect(item) {
        if (item != null) existed = true else if (existed) currentOnBack()
    }

    FlagSecureEffect(enabled = isSecret)
    val activity = LocalContext.current.findActivity()
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) {
        if (CodeGate.closesOnStop(item, activity?.isChangingConfigurations == true)) currentOnBack()
    }

    val wrap = state.codeWrapEnabled
    val content = CodeGate.content(item, loaded = state.loaded, unlocked = unlocked)
    var selectedSlot by rememberSaveable(itemId) { mutableIntStateOf(startSlot) }
    var searching by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var menuOpen by remember { mutableStateOf(false) }
    var copyText by remember { mutableStateOf<String?>(null) }

    val files = (content as? CodeContent.Files)?.files.orEmpty()
    val position = CodeFiles.positionFor(files, selectedSlot)
    val current = files.getOrNull(position)

    BackHandler(enabled = searching) {
        searching = false
        query = ""
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        contentWindowInsets = tomiContentInsets,
        snackbarHost = { TomiSnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        // A hidden item shows no name before the gate (current is null until then).
                        current?.attachment?.name.orEmpty(),
                        style = MaterialTheme.typography.titleMedium.copy(fontFamily = TomiFonts.mono),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_back))
                    }
                },
                actions = {
                    if (files.size > 1) {
                        val label = CodeFiles.positionLabel(position, files.size)
                        Text(
                            label,
                            style = MaterialTheme.typography.labelLarge,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.semantics { contentDescription = label },
                        )
                    }
                    if (current != null) {
                        IconButton(onClick = {
                            if (searching) query = ""
                            searching = !searching
                        }) {
                            Icon(Icons.Outlined.Search, contentDescription = stringResource(R.string.code_search))
                        }
                        Box {
                            IconButton(onClick = { menuOpen = true }) {
                                Icon(Icons.Outlined.MoreVert, contentDescription = stringResource(R.string.action_more))
                            }
                            DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                                DropdownMenuItem(
                                    text = { Text(stringResource(R.string.code_copy_file)) },
                                    leadingIcon = { Icon(Icons.Outlined.ContentCopy, contentDescription = null) },
                                    enabled = copyText != null,
                                    onClick = {
                                        menuOpen = false
                                        val text = copyText
                                        if (item != null && text != null) viewModel.copyCode(item, text)
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(R.string.code_wrap)) },
                                    trailingIcon = { Checkbox(checked = wrap, onCheckedChange = null) },
                                    modifier = Modifier.semantics { toggleableState = if (wrap) ToggleableState.On else ToggleableState.Off },
                                    onClick = {
                                        menuOpen = false
                                        viewModel.setCodeWrap(CodeWrap.toggled(wrap))
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(R.string.action_details)) },
                                    leadingIcon = { Icon(Icons.Outlined.Info, contentDescription = null) },
                                    onClick = {
                                        menuOpen = false
                                        onOpenDetail()
                                    },
                                )
                            }
                        }
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
    ) { padding ->
        Column(Modifier.screenContentPadding(padding).fillMaxSize()) {
            when (content) {
                CodeContent.Loading -> Unit
                CodeContent.Locked -> Message(R.string.code_locked)
                CodeContent.Empty -> Message(R.string.code_no_file)
                is CodeContent.Files -> if (item != null && current != null) {
                    if (files.size > 1) {
                        FileTabs(files, position) { selectedSlot = files[it].slot }
                    }
                    CodeBody(
                        item = item,
                        file = current,
                        viewModel = viewModel,
                        wrap = wrap,
                        searching = searching,
                        query = query,
                        onQuery = { query = it },
                        onCloseSearch = {
                            searching = false
                            query = ""
                        },
                        onTextReady = { copyText = it },
                    )
                }
            }
        }
    }
}

@Composable
private fun Message(messageRes: Int) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text(
            stringResource(messageRes),
            style = MaterialTheme.typography.bodyLarge,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(32.dp),
        )
    }
}

/** Tab bar of the code files of the item (file names), scrollable sideways. */
@Composable
private fun FileTabs(files: List<CodeFile>, selected: Int, onSelect: (Int) -> Unit) {
    LazyRow(
        contentPadding = PaddingValues(horizontal = 16.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        itemsIndexed(files) { index, file ->
            FilterChip(
                selected = index == selected,
                onClick = { onSelect(index) },
                label = {
                    Text(
                        file.attachment.name,
                        style = MaterialTheme.typography.labelLarge.copy(fontFamily = TomiFonts.mono),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                shape = CircleShape,
            )
        }
    }
}

/** What is known about the file shown. */
private sealed interface LoadedCode {
    data object Loading : LoadedCode
    class Failed(val messageRes: Int) : LoadedCode

    /** [runs]: tree-sitter colours by line ([coloured] is false, and [runs] empty, when the file could not be coloured). */
    class Ready(
        val doc: CodeDocument,
        val original: String,
        val runs: Map<Int, List<LineRun>>,
        val coloured: Boolean,
    ) : LoadedCode
}

@Composable
private fun CodeBody(
    item: TomiItem,
    file: CodeFile,
    viewModel: FeedViewModel,
    wrap: Boolean,
    searching: Boolean,
    query: String,
    onQuery: (String) -> Unit,
    onCloseSearch: () -> Unit,
    onTextReady: (String?) -> Unit,
) {
    val context = LocalContext.current
    val resolver = context.contentResolver
    val dark = MaterialTheme.colorScheme.surface.luminance() < 0.5f
    val palette = CodePalette.of(dark)
    val itemId = item.id
    val currentOnTextReady by rememberUpdatedState(onTextReady)
    val loaded by produceState<LoadedCode>(LoadedCode.Loading, itemId, file.slot, palette) {
        currentOnTextReady(null)
        // The decrypted copy of the core cache is read, never copied: bytes go to memory and nowhere else.
        val uri = viewModel.codeFile(itemId, file.slot)
        val result = if (uri == null) CodeLoadResult.Unavailable else CodeFileReader.read(resolver, uri)
        val next: LoadedCode = when (result) {
            CodeLoadResult.Unavailable -> LoadedCode.Failed(R.string.code_unavailable)
            CodeLoadResult.TooLarge -> LoadedCode.Failed(R.string.code_too_large)
            CodeLoadResult.Binary -> LoadedCode.Failed(R.string.code_binary)
            is CodeLoadResult.Text -> withContext(Dispatchers.Default) {
                val doc = CodeDocument(result.display)
                val grammar = file.grammar
                val flat = if (grammar != null && CodeLimits.canHighlight(doc.text, doc.lineCount)) {
                    TreeSitterHighlighter.highlight(context, doc.text, grammar, palette)
                } else {
                    IntArray(0)
                }
                LoadedCode.Ready(doc, result.original, CodeLines.bucketRuns(doc, flat), coloured = flat.isNotEmpty())
            }
        }
        value = next
        if (next is LoadedCode.Ready) currentOnTextReady(next.original)
    }

    when (val state = loaded) {
        LoadedCode.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            // Charter 6.3: linear progress only, never a circular spinner.
            LinearProgressIndicator(Modifier.width(160.dp))
        }
        is LoadedCode.Failed -> Message(state.messageRes)
        is LoadedCode.Ready -> ReadyCode(state, wrap, searching, query, onQuery, onCloseSearch, palette, file)
    }
}

@Composable
private fun ReadyCode(
    state: LoadedCode.Ready,
    wrap: Boolean,
    searching: Boolean,
    query: String,
    onQuery: (String) -> Unit,
    onCloseSearch: () -> Unit,
    palette: CodePalette,
    file: CodeFile,
) {
    val doc = state.doc
    val matches = remember(doc, query) { CodeSearch.find(doc.text, query) }
    var currentMatch by remember(matches) { mutableIntStateOf(CodeSearch.firstFrom(matches, 0)) }
    val matchesByLine = remember(doc, matches) { CodeLines.bucketMatches(doc, matches) }

    val listState = rememberLazyListState()
    val hScroll = rememberScrollState()
    var viewportWidthPx by remember { mutableIntStateOf(0) }

    val density = LocalDensity.current
    val measurer = rememberTextMeasurer()
    val codeStyle = remember { TextStyle(fontFamily = TomiFonts.mono, fontSize = 13.sp, lineHeight = 19.sp) }
    val charPx = remember(codeStyle, density) { measurer.measure(AnnotatedString("0".repeat(20)), codeStyle).size.width / 20f }
    val gutterChars = doc.lineCount.toString().length
    val gutterOffset = gutterChars + GUTTER_GAP_CHARS
    val contentWidthPx = (gutterOffset + doc.maxLineLength + 3) * charPx
    val lineHeight = with(density) { codeStyle.lineHeight.toDp() }

    val endPaddingPx = with(density) { WRAP_END_PADDING.toPx() }
    val lineHeightPx = with(density) { codeStyle.lineHeight.toPx() }

    // Scroll to the current occurrence.
    // Unfolded: vertically when its line is not comfortably visible, horizontally when its column is not.
    // Folded: a line can be several rows tall and there is no sideways scroll, so after bringing the line in, the row that holds
    // the occurrence is found by measuring the line at the width the screen gives it, and scrolled into view.
    LaunchedEffect(currentMatch, matches, wrap) {
        val match = matches.getOrNull(currentMatch) ?: return@LaunchedEffect
        val line = doc.lineOf(match.start)
        val visible = listState.layoutInfo.visibleItemsInfo
        if (wrap) {
            if (visible.none { it.index == line }) listState.animateScrollToItem((line - SCROLL_CONTEXT_LINES).coerceAtLeast(0))
            val info = listState.layoutInfo.visibleItemsInfo.firstOrNull { it.index == line } ?: return@LaunchedEffect
            if (charPx <= 0f || viewportWidthPx <= 0) return@LaunchedEffect
            val textWidth = CodeWrap.textWidthPx(viewportWidthPx, gutterChars * charPx, GUTTER_GAP_CHARS * charPx, endPaddingPx)
            val shown = doc.line(line).take(CodeLimits.MAX_DISPLAY_LINE_CHARS)
            val layout = measurer.measure(AnnotatedString(shown), codeStyle, constraints = Constraints(maxWidth = textWidth))
            val row = layout.getLineForOffset(doc.columnOf(match.start).coerceAtMost(shown.length))
            val layoutInfo = listState.layoutInfo
            CodeWrap.scrollDelta(
                rowTop = info.offset + layout.getLineTop(row),
                rowBottom = info.offset + layout.getLineBottom(row),
                viewportStart = layoutInfo.viewportStartOffset,
                viewportEnd = layoutInfo.viewportEndOffset,
                margin = SCROLL_CONTEXT_ROWS * lineHeightPx,
            )?.let { listState.animateScrollBy(it) }
            return@LaunchedEffect
        }
        val first = visible.firstOrNull()?.index ?: 0
        val last = visible.lastOrNull()?.index ?: 0
        if (visible.isEmpty() || line <= first || line >= last - 1) listState.animateScrollToItem((line - SCROLL_CONTEXT_LINES).coerceAtLeast(0))
        if (charPx > 0f && viewportWidthPx > 0) {
            val visibleStart = (hScroll.value / charPx).toInt() - gutterOffset
            val visibleChars = (viewportWidthPx / charPx).toInt() - gutterOffset
            CodeLines.horizontalScrollTarget(doc.columnOf(match.start), visibleStart.coerceAtLeast(0), visibleChars)
                ?.let { target -> hScroll.animateScrollTo(((target + gutterOffset) * charPx).toInt()) }
        }
    }

    Column(Modifier.fillMaxSize()) {
        if (searching) {
            SearchRow(
                query = query,
                onQuery = onQuery,
                counter = CodeSearch.counter(currentMatch, matches.size),
                hasMatches = matches.isNotEmpty(),
                onPrevious = { currentMatch = CodeSearch.previous(currentMatch, matches.size) },
                onNext = { currentMatch = CodeSearch.next(currentMatch, matches.size) },
                onClose = onCloseSearch,
            )
        }
        if (!state.coloured) {
            Text(
                stringResource(if (file.grammar == null) R.string.code_no_colours_language else R.string.code_no_colours_size),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
            )
        }
        Surface(
            color = Color(palette.background),
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            modifier = Modifier.fillMaxSize().padding(horizontal = 8.dp),
        ) {
            Box(
                Modifier
                    .fillMaxSize()
                    .onSizeChanged { viewportWidthPx = it.width }
                    // Unfolded: the lines keep their length and the whole block scrolls sideways. Folded: no sideways scroll.
                    .then(if (wrap) Modifier else Modifier.horizontalScroll(hScroll)),
            ) {
                LazyColumn(
                    state = listState,
                    contentPadding = PaddingValues(top = 8.dp, bottom = 8.dp, end = if (wrap) WRAP_END_PADDING else 0.dp),
                    modifier = (if (wrap) Modifier.fillMaxWidth() else Modifier.width(with(density) { contentWidthPx.toDp() })).fillMaxHeight(),
                ) {
                    items(count = doc.lineCount, key = { it }) { line ->
                        val lineMatches = matchesByLine[line]
                        val activeMatch = if (lineMatches != null) currentMatch else -1
                        val text = remember(doc, line, state.runs, lineMatches, activeMatch, palette) {
                            lineText(doc, line, state.runs[line], lineMatches, activeMatch, palette)
                        }
                        // Folded: the row grows with the number of folded rows (an empty line keeps one row); the number stays on top.
                        Row(if (wrap) Modifier.fillMaxWidth().heightIn(min = lineHeight) else Modifier.height(lineHeight)) {
                            Text(
                                (line + 1).toString(),
                                style = codeStyle,
                                color = Color(palette.gutter),
                                textAlign = TextAlign.End,
                                maxLines = 1,
                                softWrap = false,
                                modifier = Modifier.width(with(density) { (gutterChars * charPx).toDp() }),
                            )
                            Text(
                                text,
                                style = codeStyle,
                                color = Color(palette.text),
                                maxLines = if (wrap) Int.MAX_VALUE else 1,
                                softWrap = wrap,
                                overflow = TextOverflow.Clip,
                                modifier = (if (wrap) Modifier.weight(1f) else Modifier)
                                    .padding(start = with(density) { (GUTTER_GAP_CHARS * charPx).toDp() }),
                            )
                        }
                    }
                }
            }
        }
    }
}

/** One line as styled text: tree-sitter colours, then the search occurrences on top (text colour kept readable on the highlight). */
internal fun lineText(
    doc: CodeDocument,
    line: Int,
    runs: List<LineRun>?,
    matches: List<LineMatch>?,
    currentMatch: Int,
    palette: CodePalette,
): AnnotatedString {
    val raw = doc.line(line)
    val cut = raw.length > CodeLimits.MAX_DISPLAY_LINE_CHARS
    val shown = if (cut) raw.substring(0, CodeLimits.MAX_DISPLAY_LINE_CHARS) + "…" else raw
    val limit = shown.length
    return buildAnnotatedString {
        append(shown)
        runs?.forEach { run ->
            val end = minOf(run.endCol, limit)
            if (run.startCol < end) addStyle(SpanStyle(color = Color(run.argb)), run.startCol, end)
        }
        matches?.forEach { match ->
            val end = minOf(match.endCol, limit)
            if (match.startCol < end) {
                val background = if (match.index == currentMatch) palette.searchCurrent else palette.searchMatch
                addStyle(SpanStyle(color = Color(palette.text), background = Color(background)), match.startCol, end)
            }
        }
    }
}

@Composable
private fun SearchRow(
    query: String,
    onQuery: (String) -> Unit,
    counter: String,
    hasMatches: Boolean,
    onPrevious: () -> Unit,
    onNext: () -> Unit,
    onClose: () -> Unit,
) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.surfaceVariant,
        // Explicit: the icons of the bar (previous, next, close) keep the colour of the top bars (onSurface).
        contentColor = MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextField(
                value = query,
                // One line: a newline would make an occurrence that cannot be shown.
                onValueChange = { onQuery(it.substringBefore('\n').take(MAX_QUERY)) },
                singleLine = true,
                placeholder = { Text(stringResource(R.string.code_search_placeholder)) },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { onNext() }),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = Color.Transparent,
                    unfocusedContainerColor = Color.Transparent,
                    focusedIndicatorColor = Color.Transparent,
                    unfocusedIndicatorColor = Color.Transparent,
                ),
                modifier = Modifier.weight(1f).focusRequester(focus),
            )
            Text(
                counter,
                style = MaterialTheme.typography.labelLarge.copy(fontFamily = TomiFonts.mono),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.semantics { contentDescription = counter },
            )
            IconButton(onClick = onPrevious, enabled = hasMatches) {
                Icon(Icons.Outlined.KeyboardArrowUp, contentDescription = stringResource(R.string.code_search_previous))
            }
            IconButton(onClick = onNext, enabled = hasMatches) {
                Icon(Icons.Outlined.KeyboardArrowDown, contentDescription = stringResource(R.string.code_search_next))
            }
            IconButton(onClick = onClose) {
                Icon(Icons.Outlined.Close, contentDescription = stringResource(R.string.action_close_search))
            }
        }
    }
}

/** Reads a `content://` file of the core into memory (2 MB at most). */
internal object CodeFileReader {
    suspend fun read(resolver: ContentResolver, uri: String): CodeLoadResult = withContext(Dispatchers.IO) {
        val parsed = uri.toUri()
        // The core hands out content:// URIs of its own provider only; anything else is refused.
        if (parsed.scheme != "content") return@withContext CodeLoadResult.Unavailable
        try {
            resolver.openInputStream(parsed)?.use { CodeDecoding.read(it) } ?: CodeLoadResult.Unavailable
        } catch (_: IOException) {
            CodeLoadResult.Unavailable
        } catch (_: SecurityException) {
            CodeLoadResult.Unavailable
        }
    }
}

private const val GUTTER_GAP_CHARS = 2
private const val SCROLL_CONTEXT_LINES = 3
private const val SCROLL_CONTEXT_ROWS = 2
private val WRAP_END_PADDING = 8.dp
private const val MAX_QUERY = 200
