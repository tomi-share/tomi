package eu.shaffe.tomi

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.stateIn

/** Observes connectivity only to label queued items "waiting for network". It does not gate the sends. */
class NetworkMonitor(context: Context, scope: CoroutineScope) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)

    val isOnline: StateFlow<Boolean> = callbackFlow {
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                trySend(capabilities.isUsable())
            }

            override fun onLost(network: Network) {
                trySend(false)
            }
        }
        connectivity.registerDefaultNetworkCallback(callback)
        awaitClose { connectivity.unregisterNetworkCallback(callback) }
    }.stateIn(scope, SharingStarted.WhileSubscribed(STOP_TIMEOUT_MS), currentlyOnline())

    private fun currentlyOnline(): Boolean =
        connectivity.getNetworkCapabilities(connectivity.activeNetwork)?.isUsable() == true

    private fun NetworkCapabilities.isUsable(): Boolean =
        hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
            hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)

    private companion object {
        const val STOP_TIMEOUT_MS = 5_000L
    }
}
