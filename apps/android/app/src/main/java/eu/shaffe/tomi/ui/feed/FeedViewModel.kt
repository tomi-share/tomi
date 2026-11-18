package eu.shaffe.tomi.ui.feed

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.ReadPolicy
import eu.shaffe.tomi.core.safeCore
import eu.shaffe.tomi.core.shouldMarkReadWhenOpened
import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.AttachmentRemoval
import eu.shaffe.tomi.core.findBySlot
import eu.shaffe.tomi.core.slotOf
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.share.FileShareOutcome
import eu.shaffe.tomi.share.FileSharing
import eu.shaffe.tomi.share.ImageActionResult
import eu.shaffe.tomi.share.ImageActions
import eu.shaffe.tomi.share.ItemSharing
import eu.shaffe.tomi.share.ShareOutcome
import eu.shaffe.tomi.share.SharePayload
import eu.shaffe.tomi.share.SharedFile
import eu.shaffe.tomi.ui.render.RenderMode
import eu.shaffe.tomi.ui.render.RenderModes
import eu.shaffe.tomi.ui.viewer.ViewerActions
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class FeedUiState(
    /** Items matching the filter and the search, newest first. */
    val items: List<TomiItem> = emptyList(),
    /** The same items split into the "Pinned" section and the rest. */
    val sections: FeedSections = FeedSections(emptyList(), emptyList()),
    val allItems: List<TomiItem> = emptyList(),
    val filter: FeedFilter = FeedFilter.All,
    val query: String = "",
    val revealedIds: Set<String> = emptySet(),
    val localDeviceId: String = "",
    val isOnline: Boolean = true,
    val preferredRenderMode: RenderMode? = null,
    /** Line wrap of the code file screen (persisted preference). */
    val codeWrapEnabled: Boolean = false,
    val loaded: Boolean = false,
)

sealed interface FeedEvent {
    data object Copied : FeedEvent
    data object CopiedSecret : FeedEvent
    data object BiometricUnavailable : FeedEvent
    data object DeletedLocal : FeedEvent
    data object DeletedEverywhere : FeedEvent
    data object PinFailed : FeedEvent

    /** A call on the core failed (network, storage...). The action did not happen; nothing is fatal. */
    data object ActionFailed : FeedEvent

    /** [action] was done on [itemId] (as it was: [wasPinned]). The banner says so and offers "Undo" ([FeedViewModel.undoArchive]). */
    data class ArchiveDone(val action: ArchiveAction, val itemId: String, val wasPinned: Boolean) : FeedEvent
    data object ArchiveFailed : FeedEvent
    data object NothingToShare : FeedEvent
    data object ImageSaved : FeedEvent
    data object ImageUnavailable : FeedEvent
    data object ImageActionFailed : FeedEvent

    /**
     * ONE attachment of an item was removed (the item stays): an [image] or another file, from this device only or [everywhere].
     * When it was the last thing in the item, the core deleted the item and the event is [DeletedLocal] / [DeletedEverywhere] instead.
     */
    data class AttachmentDeleted(val image: Boolean, val everywhere: Boolean) : FeedEvent
}

class FeedViewModel(private val container: AppContainer) : ViewModel() {
    private val core = container.core

    /** The core runs its demo space: the feed shows [DemoBanner] and its way out. */
    val isDemo: StateFlow<Boolean> = core.isDemo

    /**
     * "Quitter la démo": on the container's scope, so that a restart of the activity does not cut it. No navigation here: the device is
     * then unpaired, and TomiRoot starts again from the welcome screen.
     */
    fun leaveDemo() {
        container.scope.launch { safeCore { core.leaveDemo() } }
    }

    private val filter = MutableStateFlow(FeedFilter.All)
    private val query = MutableStateFlow("")
    private val preferredMode = MutableStateFlow(container.prefs.preferredRenderMode)
    private val codeWrap = MutableStateFlow(container.prefs.codeWrapEnabled)

    /** Secrets currently shown in clear, with the item as it was when revealed (it may vanish from the core, see below). */
    private val revealed = MutableStateFlow<Map<String, TomiItem>>(emptyMap())
    private val hideJobs = mutableMapOf<String, Job>()

    private val eventChannel = Channel<FeedEvent>(Channel.BUFFERED)
    val events = eventChannel.receiveAsFlow()

    private val merged = combine(core.items, revealed) { items, revealedSnapshots ->
        // An "after first read" secret leaves the core as soon as it is read; keep it on screen until it is hidden.
        val missing = revealedSnapshots.values.filter { snapshot -> items.none { it.id == snapshot.id } }
        (items + missing).sortedByDescending { it.createdAt }
    }

    val uiState: StateFlow<FeedUiState> = combine(
        merged, filter, query, revealed, combine(container.networkMonitor.isOnline, preferredMode, codeWrap) { o, m, w -> Triple(o, m, w) },
    ) { items, f, q, rev, (online, mode, wrap) ->
        val visible = FeedFiltering.apply(items, f, q)
        FeedUiState(
            items = visible,
            sections = FeedSections.split(visible),
            allItems = items,
            filter = f,
            query = q,
            revealedIds = rev.keys,
            localDeviceId = core.localDevice.value.id,
            isOnline = online,
            preferredRenderMode = mode,
            codeWrapEnabled = wrap,
            loaded = true,
        )
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(STOP_TIMEOUT_MS), FeedUiState())

    fun setFilter(value: FeedFilter) {
        filter.value = value
    }

    fun setQuery(value: String) {
        query.value = value.take(MAX_QUERY)
    }

    fun setRenderMode(mode: RenderMode) {
        container.prefs.preferredRenderMode = mode
        preferredMode.value = mode
    }

    /** Turns the line wrap of the code file screen on or off, for every file and every later session. */
    fun setCodeWrap(enabled: Boolean) {
        container.prefs.codeWrapEnabled = enabled
        codeWrap.value = enabled
    }

    /**
     * Runs a user action that calls the core: any failure (network, storage, Keystore...) except a cancellation is reported by
     * [FeedEvent.ActionFailed] and ends the action, instead of ending the app. True when it went through.
     */
    private suspend fun attempt(block: suspend () -> Unit): Boolean =
        safeCore(onFailure = { eventChannel.send(FeedEvent.ActionFailed) }, block = block)

    /**
     * "Read" is bookkeeping done after an action that already succeeded for the user (a card opened, a text copied): when it fails,
     * the item stays unread and the next read tries again; no message, the user did nothing wrong.
     */
    private suspend fun markReadQuietly(itemId: String) {
        safeCore { core.markRead(itemId) }
    }

    /** Called when the user opens/taps a card. Secrets are only marked read once actually revealed or copied. */
    fun onCardOpened(item: TomiItem) {
        if (!item.shouldMarkReadWhenOpened(core.localDevice.value.id)) return
        viewModelScope.launch { markReadQuietly(item.id) }
    }

    /**
     * Cards that stayed on screen (see [SeenTracker]) are read. Same rule as a tap: never a hidden item (revealing it is
     * the read), an unreadable one or one we sent ourselves.
     */
    fun onItemsSeen(items: List<TomiItem>) {
        val localId = core.localDevice.value.id
        val unread = items.filter { it.shouldMarkReadWhenOpened(localId) }
        if (unread.isEmpty()) return
        viewModelScope.launch { unread.forEach { markReadQuietly(it.id) } }
    }

    fun reveal(item: TomiItem, authenticate: suspend () -> AuthOutcome) {
        viewModelScope.launch {
            when (authenticate()) {
                AuthOutcome.Success -> {
                    revealed.update { it + (item.id to item) }
                    scheduleHide(item.id)
                    // An "after first read" secret is consumed here: a failure is told, the secret is on screen anyway.
                    if (!item.read || item.readPolicy == ReadPolicy.AfterFirstRead) attempt { core.markRead(item.id) }
                }
                AuthOutcome.Unavailable -> eventChannel.send(FeedEvent.BiometricUnavailable)
                AuthOutcome.Cancelled, AuthOutcome.Failed -> Unit
            }
        }
    }

    fun hide(itemId: String) {
        hideJobs.remove(itemId)?.cancel()
        revealed.update { it - itemId }
    }

    fun hideAll() {
        hideJobs.values.forEach { it.cancel() }
        hideJobs.clear()
        revealed.value = emptyMap()
    }

    /** Copies exactly what the card shows. A hidden secret needs authentication first. */
    fun copy(item: TomiItem, mode: RenderMode?, authenticate: suspend () -> AuthOutcome) {
        viewModelScope.launch {
            if (item.secret && item.id !in revealed.value) {
                when (authenticate()) {
                    AuthOutcome.Success -> Unit
                    AuthOutcome.Unavailable -> {
                        eventChannel.send(FeedEvent.BiometricUnavailable)
                        return@launch
                    }
                    AuthOutcome.Cancelled, AuthOutcome.Failed -> return@launch
                }
            }
            val text = item.text
            val plain: String
            var html: String? = null
            if (text != null) {
                val content = RenderModes.copyContent(text, mode ?: RenderModes.resolve(text, preferredMode.value))
                plain = content.plain
                html = content.html
            } else {
                plain = item.url ?: return@launch
            }
            container.clipboard.copy(plain, html, sensitive = item.secret)
            if (!item.read || item.readPolicy == ReadPolicy.AfterFirstRead) markReadQuietly(item.id)
            eventChannel.send(if (item.secret) FeedEvent.CopiedSecret else FeedEvent.Copied)
        }
    }

    /** Pins an unpinned item, unpins a pinned one. Idempotent in the core; any failure is reported, never fatal. */
    fun togglePin(item: TomiItem) {
        viewModelScope.launch {
            safeCore(onFailure = { eventChannel.send(FeedEvent.PinFailed) }) {
                if (item.pinned) core.unpin(item.id) else core.pin(item.id)
            }
        }
    }

    /**
     * Archives or un-archives [item] through the core contract (swipe and menu), then offers "Undo" in the feedback banner.
     * Never marks the item read. The core makes archive and pin exclusive (archiving unpins). A storage error is reported,
     * never fatal.
     */
    fun applyArchiveAction(item: TomiItem, action: ArchiveAction = ArchiveSwipe.menuAction(item)) {
        if (action == ArchiveAction.Archive) hide(item.id)
        viewModelScope.launch {
            val done = safeCore(onFailure = { eventChannel.send(FeedEvent.ArchiveFailed) }) {
                ArchiveSwipe.perform(core, action, item.id)
            }
            if (done) eventChannel.send(FeedEvent.ArchiveDone(action, item.id, wasPinned = item.pinned))
        }
    }

    /** "Undo" of the banner: the inverse call on the core, restoring the pin that an archive removed. */
    fun undoArchive(done: FeedEvent.ArchiveDone) {
        viewModelScope.launch {
            safeCore(onFailure = { eventChannel.send(FeedEvent.ArchiveFailed) }) {
                ArchiveSwipe.undo(core, done.action, done.itemId, done.wasPinned)
            }
        }
    }

    /**
     * Shares the item to another app (Android `ACTION_SEND`). [launch] opens the share sheet and returns false when no
     * app can handle it. A hidden item first goes through [authenticate]; its content is only read into a payload once
     * the authentication succeeded ([ItemSharing]). The payload is passed to [launch] and dropped, never stored.
     */
    fun share(item: TomiItem, authenticate: suspend () -> AuthOutcome, launch: (SharePayload) -> Boolean) {
        viewModelScope.launch {
            when (val outcome = ItemSharing.prepare(item, authenticate)) {
                is ShareOutcome.Ready -> {
                    val opened = launch(outcome.payload)
                    // Like a copy, sharing is a read: "after first read" items are consumed, others are marked read.
                    if (opened && (!item.read || item.readPolicy == ReadPolicy.AfterFirstRead)) markReadQuietly(item.id)
                }
                ShareOutcome.NothingToShare -> eventChannel.send(FeedEvent.NothingToShare)
                ShareOutcome.BiometricUnavailable -> eventChannel.send(FeedEvent.BiometricUnavailable)
                ShareOutcome.NotAuthenticated -> Unit
            }
        }
    }

    /**
     * "Partager" of a file / image item ([FileSharing.canShare]): the attachments go to [launch] (share sheet), decrypted on
     * demand and only after [authenticate] for a hidden item. A read like the text share: marks the item read once opened.
     */
    fun shareFiles(item: TomiItem, authenticate: suspend () -> AuthOutcome, launch: (List<SharedFile>) -> Boolean) {
        viewModelScope.launch {
            when (val outcome = FileSharing.prepare(item, authenticate) { id, slot -> attachmentUriOrNull(id, slot) }) {
                is FileShareOutcome.Ready -> {
                    val opened = launch(outcome.files)
                    if (opened && (!item.read || item.readPolicy == ReadPolicy.AfterFirstRead)) markReadQuietly(item.id)
                }
                FileShareOutcome.NothingToShare -> eventChannel.send(FeedEvent.NothingToShare)
                FileShareOutcome.BiometricUnavailable -> eventChannel.send(FeedEvent.BiometricUnavailable)
                FileShareOutcome.NotAuthenticated -> Unit
            }
        }
    }

    /** Copies only the link of a composite item (text + url). Refused for a hidden item: its link goes through the gate of [copy]. */
    fun copyLink(item: TomiItem) {
        if (item.secret) return
        val url = item.url ?: return
        viewModelScope.launch {
            container.clipboard.copy(url, null, sensitive = false)
            eventChannel.send(FeedEvent.Copied)
        }
    }

    /**
     * Opens the one attachment of [item] with the system viewer ([launch] gets the `content://` URI and the MIME type
     * and returns false when no app can open it). Anything else (several attachments, URI not available, no viewer):
     * [onUnavailable], which the screen maps to the detail. Both callbacks run on the main thread.
     */
    fun openAttachment(item: TomiItem, launch: (uri: String, mime: String) -> Boolean, onUnavailable: () -> Unit) {
        val attachment = item.attachments.singleOrNull()
        if (attachment == null || item.secret) {
            onUnavailable()
            return
        }
        viewModelScope.launch {
            // The slot of the one attachment left: it is not 0 once the others were removed.
            val uri = attachmentUriOrNull(item.id, item.attachments.slotOf(0))
            if (uri == null || !launch(uri, attachment.mime)) onUnavailable()
        }
    }

    private val imageActions = ImageActions { itemId, slot -> attachmentUriOrNull(itemId, slot) }

    private suspend fun attachmentUriOrNull(itemId: String, slot: Int): String? = try {
        core.attachmentUri(itemId, slot)
    } catch (e: CancellationException) {
        throw e
    } catch (_: Exception) {
        null
    }

    /**
     * Authentication to open the image viewer on [item] (hidden item only, same gate and 2-minute window as "Afficher").
     * Returns true when the viewer may display the images. Failures are reported once; a cancelled prompt is silent.
     */
    suspend fun authorizeImageViewer(item: TomiItem, authenticate: suspend () -> AuthOutcome): Boolean {
        val result = imageActions.authorizeView(item, authenticate)
        reportImageResult(result)
        return result == ImageActionResult.Done
    }

    /**
     * File (`content://`, decrypted by the core into its cache on demand) of one page of the viewer. For a hidden item this
     * ENFORCES the gate: it returns null, without asking the core to decrypt anything, unless the biometric window opened by
     * [authorizeImageViewer] (the same 2 minutes as "Afficher") is still valid. A page that loads after the window closed
     * (screen off, 2 minutes elapsed) therefore shows "unavailable" instead of a clear file.
     */
    suspend fun imageFile(itemId: String, slot: Int): String? = gatedFile(itemId, slot)

    /**
     * Authentication to open the code screen on [item] (hidden item only, same gate and 2-minute window as "Afficher", through
     * the same [ImageActions] rule as the image viewer). Returns true when the screen may read and display the files: nothing is
     * read or decoded before that. A missing screen lock is reported once; a cancelled prompt is silent.
     */
    suspend fun authorizeCodeScreen(item: TomiItem, authenticate: suspend () -> AuthOutcome): Boolean {
        val result = imageActions.authorizeView(item, authenticate)
        if (result == ImageActionResult.BiometricUnavailable) eventChannel.send(FeedEvent.BiometricUnavailable)
        return result == ImageActionResult.Done
    }

    /**
     * File (`content://`, decrypted by the core into its cache on demand) of one code file. Same enforcement as [imageFile]: for a
     * hidden item, null unless the biometric window opened by [authorizeCodeScreen] is still valid.
     */
    suspend fun codeFile(itemId: String, slot: Int): String? = gatedFile(itemId, slot)

    /** The item as the feed knows it now (including a revealed secret that left the core); null when unknown. */
    private fun knownItem(itemId: String): TomiItem? = uiState.value.allItems.firstOrNull { it.id == itemId }

    private suspend fun gatedFile(itemId: String, slot: Int): String? =
        imageActions.fileForView(knownItem(itemId), slot) { container.biometricGate.isWindowOpen() }

    /**
     * "Copier le fichier" of the code screen: puts [text] on the clipboard. A hidden item goes through the sensitive clipboard
     * (flagged, cleared after 30 s); the screen only calls this once the gate has passed.
     */
    fun copyCode(item: TomiItem, text: String) {
        viewModelScope.launch {
            container.clipboard.copy(text, null, sensitive = item.secret)
            if (!item.read || item.readPolicy == ReadPolicy.AfterFirstRead) markReadQuietly(item.id)
            eventChannel.send(if (item.secret) FeedEvent.CopiedSecret else FeedEvent.Copied)
        }
    }

    /**
     * "Enregistrer dans la galerie". A hidden item first goes through [authenticate]; the file is neither decrypted nor
     * written before it succeeded. [write] copies the file behind the URI into the gallery and returns true on success.
     */
    fun saveImage(
        item: TomiItem,
        slot: Int,
        authenticate: suspend () -> AuthOutcome,
        write: suspend (uri: String, attachment: Attachment) -> Boolean,
    ) {
        // Defence in depth: the menu does not offer it for a hidden item (ViewerActions), and nothing is decrypted if it is called anyway.
        if (!ViewerActions.canSaveToGallery(item)) return
        viewModelScope.launch {
            val result = imageActions.save(item, slot, authenticate, write)
            if (result == ImageActionResult.Done) eventChannel.send(FeedEvent.ImageSaved) else reportImageResult(result)
        }
    }

    /** "Partager" an image to another app. Same gate as [saveImage]; [launch] opens the share sheet. */
    fun shareImage(
        item: TomiItem,
        slot: Int,
        authenticate: suspend () -> AuthOutcome,
        launch: (uri: String, attachment: Attachment) -> Boolean,
    ) {
        viewModelScope.launch {
            val result = imageActions.share(item, slot, authenticate, launch)
            reportImageResult(result)
        }
    }

    private suspend fun reportImageResult(result: ImageActionResult) {
        when (result) {
            ImageActionResult.Done -> Unit
            ImageActionResult.Unavailable -> eventChannel.send(FeedEvent.ImageUnavailable)
            ImageActionResult.Failed -> eventChannel.send(FeedEvent.ImageActionFailed)
            ImageActionResult.BiometricUnavailable -> eventChannel.send(FeedEvent.BiometricUnavailable)
            ImageActionResult.NotAuthenticated -> Unit
        }
    }

    fun onOpened(item: TomiItem) {
        viewModelScope.launch { if (!item.read && !item.secret) markReadQuietly(item.id) }
    }

    /** "Supprimé" is only announced when the core did it; a failure says so instead, and the item stays. */
    fun deleteLocal(itemId: String) {
        hide(itemId)
        viewModelScope.launch {
            if (attempt { core.deleteLocal(itemId) }) eventChannel.send(FeedEvent.DeletedLocal)
        }
    }

    fun deleteEverywhere(itemId: String) {
        hide(itemId)
        viewModelScope.launch {
            if (attempt { core.deleteEverywhere(itemId) }) eventChannel.send(FeedEvent.DeletedEverywhere)
        }
    }

    private val removalEverywhere = MutableStateFlow(false)

    /**
     * True when the relay accepts "remove this file everywhere" ([TomiCore.supportsAttachmentRemoval]); the screens offer that scope
     * only then. False until [refreshAttachmentRemovalSupport] heard the core.
     */
    val attachmentRemovalEverywhere: StateFlow<Boolean> = removalEverywhere

    /** Re-reads the capability (a field of the core, not the network). Called when a screen that offers the removal opens. */
    fun refreshAttachmentRemovalSupport() {
        viewModelScope.launch {
            removalEverywhere.value = try {
                core.supportsAttachmentRemoval()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                false
            }
        }
    }

    /**
     * Removes the attachment of [slot] (its [Attachment.slot], never its position) from [item], from this device only or
     * [everywhere]. The others stay; the core deletes the whole item, with the same scope, when nothing else is left in it
     * ([AttachmentRemovals.feedback] says which message). A failure is reported and nothing is announced.
     */
    fun deleteAttachment(item: TomiItem, slot: Int, everywhere: Boolean) {
        val attachment = item.attachments.findBySlot(slot) ?: return
        viewModelScope.launch {
            var removal: AttachmentRemoval? = null
            if (attempt { removal = core.deleteAttachment(item.id, slot, everywhere) }) {
                val result = removal ?: return@launch
                if (result == AttachmentRemoval.ItemDeleted) hide(item.id)
                eventChannel.send(AttachmentRemovals.feedback(result, attachment.isImage, everywhere))
            }
        }
    }

    private fun scheduleHide(itemId: String) {
        hideJobs.remove(itemId)?.cancel()
        hideJobs[itemId] = viewModelScope.launch {
            delay(AUTO_HIDE_MS)
            hide(itemId)
        }
    }

    override fun onCleared() {
        hideAll()
    }

    private companion object {
        const val AUTO_HIDE_MS = 30_000L
        const val STOP_TIMEOUT_MS = 5_000L
        const val MAX_QUERY = 200
    }
}
