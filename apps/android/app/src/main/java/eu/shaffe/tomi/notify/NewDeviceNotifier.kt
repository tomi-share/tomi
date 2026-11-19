package eu.shaffe.tomi.notify

import eu.shaffe.tomi.core.TomiCore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * "A device joined your space" alert, from the device list that the core already keeps in step with the relay. It is a detection
 * control: if the owner did not add that device, they can revoke it at once. Nothing is posted while the app is on screen (the person
 * who added the device is looking at it), nor for the list this device receives while it is itself being paired.
 */
class NewDeviceNotifier(
    private val core: TomiCore,
    private val notifications: IncomingNotifications,
    private val scope: CoroutineScope,
    private val isAppVisible: () -> Boolean,
    nowMs: () -> Long = { System.nanoTime() / NANOS_PER_MS },
) {
    private val detector = NewDeviceDetector()
    private val grace = PairingGrace(nowMs)

    fun start(): Job = scope.launch {
        core.devices.collect { list ->
            val importing = grace.isImporting(core.isPaired.value)
            val added = detector.onDevices(list, importing)
            if (added.isNotEmpty() && !isAppVisible()) added.forEach { notifications.showDeviceAdded(it) }
        }
    }

    private companion object {
        const val NANOS_PER_MS = 1_000_000L
    }
}
