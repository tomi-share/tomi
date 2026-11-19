package eu.shaffe.tomi.ui.send

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.R
import eu.shaffe.tomi.deeplink.DeepLink
import eu.shaffe.tomi.send.CancelOutcome
import eu.shaffe.tomi.send.Submission
import eu.shaffe.tomi.share.ExpiryChoice
import eu.shaffe.tomi.share.ExpiryPolicy
import eu.shaffe.tomi.share.SecretDetector
import eu.shaffe.tomi.share.SharePayloadParser
import eu.shaffe.tomi.share.ShareInput
import eu.shaffe.tomi.share.SharedStream
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** Why the "secret" switch is on, shown under it. */
enum class SecretHint { None, SourceSensitive, Heuristic }

/**
 * State of the compose screen. The clipboard is read only by [onPasted], which the screen calls from the
 * click handler of the "Paste" button.
 */
class SendViewModel(private val container: AppContainer) : ViewModel() {

    var title by mutableStateOf("")
        private set
    var text by mutableStateOf("")
        private set
    var secret by mutableStateOf(false)
        private set
    var secretLocked by mutableStateOf(false)
        private set
    var secretHint by mutableStateOf(SecretHint.None)
        private set
    var expiry by mutableStateOf(ExpiryPolicy.DEFAULT)
        private set
    var submission by mutableStateOf<Submission?>(null)
        private set

    /**
     * One-shot: an item was just sent, the compose screen hands over to the feed (where the item shows up, with the
     * "Sent / Undo" banner). Consumed by the screen with [consumeLeaveAfterSend].
     */
    var leaveAfterSend by mutableStateOf(false)
        private set

    /** Files chosen with "Attach"; copied to private storage only when the item is sent. */
    var attachments by mutableStateOf<List<SharedStream>>(emptyList())
        private set

    /** Names of the other devices of the space ("To PC-Bureau and PC-Maison"). */
    val recipients: StateFlow<List<String>> = container.core.devices
        .map { devices -> devices.filterNot { it.isThisDevice }.map { it.name } }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(STOP_TIMEOUT_MS), emptyList())

    /** Transient message (string resource id), consumed by the screen. */
    var notice by mutableStateOf<Int?>(null)
        private set

    private var pastedHtml: String? = null
    private var sending = false

    /** What was just sent, for "Undo": dropped by itself at the end of the undo window, whatever the screen does. */
    private val undoCopy = UndoCopy<Snapshot>(viewModelScope)

    private class Snapshot(
        val title: String,
        val text: String,
        val html: String?,
        val streams: List<SharedStream>,
        val secret: Boolean,
        val locked: Boolean,
        val hint: SecretHint,
        val expiry: ExpiryChoice,
    )

    /**
     * `tomi-share://send` pre-fill: only fills the fields, never sends. A draft in progress is kept: the link is appended to it
     * ([DeepLinkPrefill]), and the draft stays as it is (secret switch, attachments, expiry included).
     */
    fun prefill(link: DeepLink.Send) {
        val merged = DeepLinkPrefill.apply(title, text, attachments.isNotEmpty(), link)
        if (merged.keepsDraft) {
            // A draft in progress is never reset: not even when the link adds nothing (secret switch, attachments and expiry stay).
            if (merged.appended) {
                title = merged.title
                text = merged.text
                pastedHtml = null
                notice = R.string.notice_link_appended_to_draft
            }
            return
        }
        submission = null
        undoCopy.clear()
        title = merged.title
        attachments = emptyList()
        text = merged.text
        pastedHtml = null
        resetSecret()
        notice = if (link.droppedParameters) R.string.notice_params_dropped else null
    }

    fun onTitleChange(value: String) {
        title = value.take(SharePayloadParser.MAX_TITLE)
    }

    fun onTextChange(value: String) {
        text = value.take(SharePayloadParser.MAX_TEXT)
        pastedHtml = null
        if (text.isEmpty()) resetSecret()
    }

    fun onSecretChange(value: Boolean) {
        if (secretLocked && !value) return
        secret = value
        expiry = ExpiryPolicy.onContentChanged(expiry, value, attachments.isNotEmpty())
        if (!value) secretHint = SecretHint.None
    }

    fun onExpiryChange(choice: ExpiryChoice) {
        if (ExpiryPolicy.isAllowed(choice, secret, attachments.isNotEmpty())) expiry = choice
    }

    /**
     * Receives what the user just pasted by tapping "Paste".
     * [sensitive] is the clip's EXTRA_IS_SENSITIVE flag: the content is then a secret, and the switch is locked on.
     */
    fun onPasted(plain: String?, html: String?, sensitive: Boolean) {
        val clean = plain?.take(SharePayloadParser.MAX_TEXT)
        if (clean.isNullOrEmpty()) {
            notice = R.string.notice_paste_empty
            return
        }
        val wasEmpty = text.isBlank()
        text = if (wasEmpty) clean else (text + "\n" + clean).take(SharePayloadParser.MAX_TEXT)
        pastedHtml = if (wasEmpty) html else null
        when {
            sensitive -> {
                secret = true
                secretLocked = true
                secretHint = SecretHint.SourceSensitive
                expiry = ExpiryPolicy.onContentChanged(expiry, true, attachments.isNotEmpty())
            }
            SecretDetector.looksLikeSecret(clean) && !secret -> {
                secret = true
                secretHint = SecretHint.Heuristic
                expiry = ExpiryPolicy.onContentChanged(expiry, true, attachments.isNotEmpty())
            }
        }
    }

    fun onAttach(streams: List<SharedStream>) {
        attachments = (attachments + streams).distinctBy { it.uri }.take(MAX_ATTACHMENTS)
        // "After first read" would destroy the attached file as soon as it is consulted: it is not offered with a file.
        expiry = ExpiryPolicy.onContentChanged(expiry, secret, attachments.isNotEmpty())
    }

    fun onRemoveAttachment(index: Int) {
        attachments = attachments.filterIndexed { i, _ -> i != index }
    }

    fun consumeNotice() {
        notice = null
    }

    fun consumeLeaveAfterSend() {
        leaveAfterSend = false
    }

    val canSend: Boolean get() = (title.isNotBlank() || text.isNotBlank() || attachments.isNotEmpty()) && !sending

    fun send() {
        if (!canSend) return
        sending = true
        val input = ShareInput(
            subject = title,
            text = text,
            htmlText = pastedHtml,
            streams = attachments,
            markedSensitive = secretLocked,
        )
        val parsed = SharePayloadParser.parse(input, container.ownAuthorities)
        if (parsed.isEmpty) {
            sending = false
            return
        }
        val current = Snapshot(title, text, pastedHtml, attachments, secret, secretLocked, secretHint, expiry)
        viewModelScope.launch {
            try {
                val sent = container.sendCoordinator.submit(parsed, secret, expiry)
                // `submission` first: a deadline already over expires on the spot and must be the last word on it.
                submission = sent
                undoCopy.keep(current, sent.deadline) { submission = null }
                title = ""
                text = ""
                pastedHtml = null
                attachments = emptyList()
                resetSecret()
                leaveAfterSend = true
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // Any failure of staging or of the core (TomiCoreException, I/O, ...): keep what the user typed.
                notice = R.string.notice_send_failed
            } finally {
                sending = false
            }
        }
    }

    /**
     * Cancels the pending send and gives the text back for editing, or says "too late" if the item already left
     * (the core decides, see [eu.shaffe.tomi.send.SendCoordinator.cancel]). Nothing is deleted in that case.
     */
    fun undo() {
        val sent = submission ?: return
        val saved = undoCopy.value
        viewModelScope.launch {
            val outcome = try {
                container.sendCoordinator.cancel(sent)
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                null
            }
            submission = null
            undoCopy.clear()
            when (outcome) {
                CancelOutcome.Cancelled -> if (saved != null) restore(saved)
                CancelOutcome.TooLate -> notice = R.string.notice_too_late
                null -> notice = R.string.notice_undo_failed
            }
        }
    }

    /**
     * The undo window is over: the item is on its way, and the copy of the text kept for "Undo" (a secret, maybe) is dropped.
     * The view model does this itself at the deadline ([UndoCopy]); the banner calls it too, which changes nothing the second time.
     */
    fun onBannerTimeout() {
        submission = null
        undoCopy.clear()
    }

    override fun onCleared() {
        undoCopy.clear()
    }

    private fun restore(saved: Snapshot) {
        title = saved.title
        text = saved.text
        pastedHtml = saved.html
        attachments = saved.streams
        secret = saved.secret
        secretLocked = saved.locked
        secretHint = saved.hint
        expiry = saved.expiry
    }

    private fun resetSecret() {
        secret = false
        secretLocked = false
        secretHint = SecretHint.None
        expiry = ExpiryPolicy.DEFAULT
    }

    private companion object {
        const val MAX_ATTACHMENTS = 20
        const val STOP_TIMEOUT_MS = 5_000L
    }
}
