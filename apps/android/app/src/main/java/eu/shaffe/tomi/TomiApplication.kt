package eu.shaffe.tomi

import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.SystemClock
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.ProcessLifecycleOwner
import eu.shaffe.tomi.core.DeliveryState
import eu.shaffe.tomi.core.DepartureRetrier
import eu.shaffe.tomi.core.CoreLinkReader
import eu.shaffe.tomi.core.DeviceNames
import eu.shaffe.tomi.core.LinkReader
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.core.isOutgoing
import eu.shaffe.tomi.notify.IncomingNotifications
import eu.shaffe.tomi.notify.IncomingNotifier
import eu.shaffe.tomi.notify.NewDeviceNotifier
import eu.shaffe.tomi.notify.NotificationChannels
import eu.shaffe.tomi.security.AuthWindow
import eu.shaffe.tomi.security.BiometricGate
import eu.shaffe.tomi.security.PlaintextPurgeWorker
import eu.shaffe.tomi.security.SensitiveClipboard
import eu.shaffe.tomi.send.FileAttachmentStaging
import eu.shaffe.tomi.send.SendCoordinator
import eu.shaffe.tomi.send.SendScheduling
import eu.shaffe.tomi.send.WorkManagerSendScheduler
import eu.shaffe.tomi.share.OwnAuthorityCheck
import eu.shaffe.tomi.share.PackageManagerOwnAuthorityCheck
import eu.shaffe.tomi.share.SharingShortcut
import eu.shaffe.tomi.ui.code.TomiCodeHighlightPlugin
import eu.shaffe.tomi.ui.feed.clearRenderCaches
import eu.shaffe.tomi.ui.render.warmUpTextRendering
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File


class TomiApplication : Application() {
    private val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /**
     * The opening of the core (Android Keystore, SQLite, first read of the state) runs OFF the main thread ([CoreStartup]); the screens wait
     * for it, or show "Données illisibles" with a way out when it fails. Nothing else is created before it is done.
     */
    val startup = CoreStartup(
        scope = appScope,
        io = Dispatchers.IO,
        main = Dispatchers.Main,
        open = {
            // Clear copies of shared files left by a process that died mid-send: gone before anything can stage a new one.
            FileAttachmentStaging.purgeAll(cacheDir)
            // Only a failure of the opening itself is "unreadable data". Once the core is open, a failure of the container's
            // constructor is a bug of the code: StartupBug crashes instead of offering a retry next to a core that is still open.
            val core = CoreFactory.real(this, appScope)
            try {
                AppContainer(this, appScope, core)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                throw StartupBug(e)
            }
        },
        onReady = { it.start() },
        wipe = { CoreFactory.wipeCoreData(this) },
    )

    /** The container, once the core is open. Only call it from a screen that [startup] has declared ready. */
    val container: AppContainer
        get() = checkNotNull(containerOrNull) { "the core is not open" }

    val containerOrNull: AppContainer? get() = (startup.state.value as? StartupState.Ready)?.value

    /** For what runs without a screen (notification, share target, background job): waits for the core, null when it cannot open. */
    suspend fun awaitContainer(): AppContainer? = startup.await()

    override fun onCreate() {
        super.onCreate()
        startup.begin()
        // While the core opens: the first use of the text rendering code, off the main thread (see warmUpTextRendering).
        appScope.launch { warmUpTextRendering() }
    }
}

/** How long the app stays in the background before the decrypted attachments are erased (another app may still be reading one). */
private const val PURGE_DELAY_MS = 60_000L

/** Manual dependency container. The core is the Rust one ([CoreFactory.real], through UniFFI), opened by [CoreStartup]. */
class AppContainer(private val app: Application, val scope: CoroutineScope, val core: TomiCore) {
    /** Reads pairing and creation links, and relay addresses: the reading of the core, shared by the three platforms. */
    val links: LinkReader = CoreLinkReader
    val packageName: String get() = app.packageName
    val prefs = UserPrefs(app)
    /** Real-time clock (keeps counting in deep sleep, immune to wall-clock changes), unlike `System.nanoTime()`. */
    private val authWindow = AuthWindow(nowMs = { SystemClock.elapsedRealtime() })
    val biometricGate = BiometricGate(authWindow)
    val clipboard = SensitiveClipboard(app)

    /** Answers "does this content:// authority belong to Tomi?" by asking the PackageManager. */
    val ownAuthorities: OwnAuthorityCheck = PackageManagerOwnAuthorityCheck(app.packageName) { authority ->
        app.packageManager.resolveContentProvider(authority, 0)?.packageName
    }
    val networkMonitor = NetworkMonitor(app, scope)
    val scheduler: SendScheduling = WorkManagerSendScheduler(app)
    val sendCoordinator = SendCoordinator(core, scheduler, FileAttachmentStaging(app))
    private val notifications = IncomingNotifications(app)

    /** Whether the app is in the foreground (ON_START .. ON_STOP of the process), set from [start]. */
    private val foreground = MutableStateFlow(false)

    /** The automatic attempts of a pending departure (spec/api-v1.md 7.9.4). */
    val departureRetrier = DepartureRetrier(
        core = core,
        scope = scope,
        online = networkMonitor.isOnline,
        foreground = foreground,
        nowMs = { SystemClock.elapsedRealtime() },
    )

    private val leftSpaceCleanup = LeftSpaceCleanup(
        prefs = prefs,
        clipboard = clipboard,
        cancelNotifications = { notifications.cancelAll() },
        closeBiometricWindow = { authWindow.invalidate() },
        deleteFiles = {
            File(app.cacheDir, FileAttachmentStaging.ROOT).deleteRecursively()
            TomiCodeHighlightPlugin.clearCache()
            // What the screens keep in memory about the items of the space that was left: decoded thumbnails, laid out heights.
            clearRenderCaches()
        },
        cancelSends = { WorkManagerSendScheduler.cancelAll(app) },
    )

    /** The name this phone proposes for itself when it creates a space (the core's rule decides which candidate is valid). */
    fun defaultDeviceName(): String = DeviceNames.choose(CoreFactory.deviceNameCandidates(app), core::isValidDeviceName)

    /** A window of the app got or lost focus (MainActivity): the clipboard can be checked only while it has it. */
    fun onWindowFocusChanged(hasFocus: Boolean) {
        clipboard.windowHasFocus = hasFocus
        if (!hasFocus) return
        // The clipboard metadata is only readable while we have focus: this is when an overdue secret can be checked.
        clipboard.clearIfStillOurs()
        leftSpaceCleanup.onWindowFocused()
    }

    fun start() {
        invalidateBiometricWindowWhenScreenTurnsOff()
        NotificationChannels.create(app)
        trackForeground()
        keepSharingShortcutWithPairing()
        cleanUpAfterLeaving()
        departureRetrier.start()
        IncomingNotifier(
            core = core,
            notifications = notifications,
            scope = scope,
            isAppVisible = { ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED) },
        ).start()
        NewDeviceNotifier(
            core = core,
            notifications = notifications,
            scope = scope,
            isAppVisible = { ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED) },
        ).start()
        resumeQueuedSends()
        settlePairingAndSync()
        keepLiveSyncWhileVisible()
        purgeDecryptedFilesInBackground()
    }

    /**
     * The decrypted copies of the attachments (opened in the viewer, the code screen, another app) are erased once the app has stayed
     * in the background for [PURGE_DELAY_MS]. Not at once: "Ouvrir avec" and "Partager" put the other app in front,
     * and it reads the file after Tomi stopped. Coming back cancels the countdown; the core decrypts again on demand.
     */
    private fun purgeDecryptedFilesInBackground() {
        scope.launch {
            foreground.collectLatest { visible ->
                if (visible) {
                    PlaintextPurgeWorker.cancel(app)
                    return@collectLatest
                }
                // The process may die during the grace below (low memory): a job restarts it and purges anyway.
                PlaintextPurgeWorker.schedule(app)
                delay(PURGE_DELAY_MS)
                try {
                    core.purgePlaintextCache()
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    // A file still open elsewhere: the rest is erased, the core erases it at the next start.
                }
            }
        }
    }

    /**
     * The real-time channel to the relay is open while the app is in the foreground AND paired, closed otherwise. Without it nothing
     * fetches what another device sends after the start-up sync: an item would only show up at the next launch. Opening it also
     * catches up at once (the core syncs right after the relay says "ready"), so coming back to the app shows what arrived meanwhile.
     */
    private fun keepLiveSyncWhileVisible() {
        scope.launch {
            combine(foreground, core.isPaired) { isVisible, paired -> isVisible && paired }
                .distinctUntilChanged()
                .collect { on -> if (on) core.startLiveSync() else core.stopLiveSync() }
        }
    }

    /** Follows the foreground of the process into [foreground]. Must be called on the main thread (lifecycle observers): from onCreate. */
    private fun trackForeground() {
        val lifecycle = ProcessLifecycleOwner.get().lifecycle
        foreground.value = lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
        lifecycle.addObserver(
            LifecycleEventObserver { _, event ->
                when (event) {
                    Lifecycle.Event.ON_START -> foreground.value = true
                    Lifecycle.Event.ON_STOP -> {
                        foreground.value = false
                        // The 2-minute authentication must not outlive the app session: whoever is handed the phone after the user
                        // switched app must be asked again (the screen-off receiver only covers a locked phone). A BiometricPrompt
                        // or a rotation does not stop the process, so the prompt itself never closes the window it just opened.
                        authWindow.invalidate()
                    }
                    else -> Unit
                }
            },
        )
    }

    /**
     * A pending departure is continued first (spec/api-v1.md 7.9.4, before the pairing): within 15 minutes of the gesture the core sends
     * the request again, later it only verifies; a proof erases this device (the screens restart, see cleanUpAfterLeaving), anything
     * transient starts the back-off of [departureRetrier]. Then a pairing or a space creation that a stop interrupted is settled
     * (spec/pairing-v1.md 11.4, 10.2), and, once paired, a first sync ("at start-up", [TomiCore.sync]). None can fail the start.
     */
    private fun settlePairingAndSync() {
        scope.launch {
            // Never throws (TomiCore contract); what is pending stays visible in the banner of the feed.
            departureRetrier.settleAtStart()
            try {
                core.resumePairing()
                if (core.isPaired.value) core.sync()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // Offline or refused: the next start (or the next sync) tries again.
            }
        }
    }

    /** Tomi is a direct share target only while this device is in a space: published when it joins one, removed when it leaves. */
    private fun keepSharingShortcutWithPairing() {
        scope.launch(Dispatchers.IO) {
            core.isPaired.collect { paired ->
                // A refused shortcut (system quota, launcher gone) must not end the collection, nor the app.
                try {
                    if (paired) SharingShortcut.publish(app) else SharingShortcut.remove(app)
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    // Direct share target is a convenience: Tomi stays in the share sheet.
                }
            }
        }
    }

    /**
     * Once this device has left its space (a proof of the relay, or "Effacer quand même"), what the app keeps outside the core goes too
     * ([LeftSpaceCleanup]). The core erased its own data; the screens restart from the welcome screen (TomiRoot, which also drops every
     * view model and what they showed). Done whenever the device is not paired, the first value at start-up included.
     */
    private fun cleanUpAfterLeaving() {
        scope.launch {
            core.isPaired.collect { paired -> if (!paired) withContext(Dispatchers.IO) { leftSpaceCleanup.run() } }
        }
    }

    /**
     * The 2-minute biometric window must not survive a locked phone: it is closed as soon as the screen turns off (and when the app
     * goes to the background, see [trackForeground]).
     * ACTION_SCREEN_OFF can only be received by a receiver registered at run time; it is a protected system broadcast,
     * so the receiver is not exported.
     */
    private fun invalidateBiometricWindowWhenScreenTurnsOff() {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (intent.action == Intent.ACTION_SCREEN_OFF) authWindow.invalidate()
            }
        }
        ContextCompat.registerReceiver(app, receiver, IntentFilter(Intent.ACTION_SCREEN_OFF), ContextCompat.RECEIVER_NOT_EXPORTED)
    }

    /** After a restart, makes sure every item still waiting in the outbox has a pending dispatch job. */
    private fun resumeQueuedSends() {
        scope.launch {
            val localId = core.localDevice.value.id
            core.items.first()
                .filter { it.isOutgoing(localId) && it.deliveries.any { d -> d.state == DeliveryState.Queued } }
                .forEach { scheduler.ensureScheduled(it.id) }
        }
    }
}
