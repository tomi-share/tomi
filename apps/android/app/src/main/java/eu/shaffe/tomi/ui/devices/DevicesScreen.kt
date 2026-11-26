package eu.shaffe.tomi.ui.devices

import android.text.format.DateUtils
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.Devices
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.LinkOff
import androidx.compose.material.icons.outlined.PersonAdd
import androidx.compose.material.icons.outlined.PhoneAndroid
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.ui.graphics.Color
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.Device
import eu.shaffe.tomi.core.DeviceInactivity
import eu.shaffe.tomi.core.DevicePlatform
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.core.safeCore
import eu.shaffe.tomi.security.AuthOutcome
import eu.shaffe.tomi.ui.common.TomiSnackbarHost
import eu.shaffe.tomi.ui.feed.RotationStalledBanner
import eu.shaffe.tomi.ui.common.showFeedback
import eu.shaffe.tomi.ui.common.rememberAuthenticator
import eu.shaffe.tomi.ui.common.screenContentPadding
import eu.shaffe.tomi.ui.common.tomiContentInsets
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import java.time.Instant

/** What the screen tells the user after an action on a device that did not go through. */
enum class DevicesEvent { RenameFailed, RevokeFailed }

class DevicesViewModel(private val core: TomiCore) : ViewModel() {
    val devices: StateFlow<List<Device>?> = core.devices
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /** The key rotation after a revocation keeps failing: the screen says the removed device may still hold the current key. */
    val rotationStalled: StateFlow<Boolean> = core.rotationStalled

    private val busyState = MutableStateFlow(false)

    /** True while a revocation runs: it rotates the space key at the relay, which takes several seconds. */
    val busy: StateFlow<Boolean> = busyState.asStateFlow()

    private val eventChannel = Channel<DevicesEvent>(Channel.BUFFERED)
    val events = eventChannel.receiveAsFlow()

    fun rename(id: String, name: String) {
        viewModelScope.launch { safeCore(onFailure = { eventChannel.send(DevicesEvent.RenameFailed) }) { core.renameDevice(id, name) } }
    }

    fun revoke(id: String) {
        if (!busyState.compareAndSet(expect = false, update = true)) return
        viewModelScope.launch {
            try {
                safeCore(onFailure = { eventChannel.send(DevicesEvent.RevokeFailed) }) { core.revokeDevice(id) }
            } finally {
                busyState.value = false
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DevicesScreen(viewModel: DevicesViewModel, onBack: () -> Unit, onAddDevice: () -> Unit) {
    val revokeUnavailable = stringResource(R.string.devices_revoke_unavailable)
    val renameFailed = stringResource(R.string.devices_rename_failed)
    val revokeFailed = stringResource(R.string.devices_revoke_failed)
    val devices by viewModel.devices.collectAsStateWithLifecycle()
    val busy by viewModel.busy.collectAsStateWithLifecycle()
    val rotationStalled by viewModel.rotationStalled.collectAsStateWithLifecycle()
    val authenticator = rememberAuthenticator()
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var renaming by remember { mutableStateOf<Device?>(null) }
    var revoking by remember { mutableStateOf<Device?>(null) }

    LaunchedEffect(viewModel) {
        viewModel.events.collect { event ->
            snackbar.showFeedback(if (event == DevicesEvent.RenameFailed) renameFailed else revokeFailed)
        }
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
                title = { Text(stringResource(R.string.devices_title)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.action_back))
                    }
                },
                actions = {
                    IconButton(onClick = onAddDevice) {
                        Icon(Icons.Outlined.PersonAdd, contentDescription = stringResource(R.string.devices_add))
                    }
                },
            )
        },
        snackbarHost = { TomiSnackbarHost(snackbar) },
        contentWindowInsets = tomiContentInsets,
    ) { padding ->
        Box(Modifier.screenContentPadding(padding).fillMaxSize()) {
            // Loaded but empty: a centred message instead of a blank page (the list itself stays top-aligned).
            if (devices?.isEmpty() == true) {
                Column(
                    Modifier.align(Alignment.Center).padding(32.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Icon(
                        Icons.Outlined.Devices,
                        contentDescription = null,
                        modifier = Modifier.size(56.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(
                        stringResource(R.string.devices_empty),
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                }
            }
            Column {
                if (rotationStalled) RotationStalledBanner()
                if (busy) {
                    // A revocation rotates the space key at the relay: several seconds. Linear bar (charter 6.3), nothing near the symbol.
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    Text(
                        stringResource(R.string.devices_revoking),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
                LazyColumn {
                    items(devices.orEmpty(), key = { it.id }) { device ->
                        ListItem(
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                            leadingContent = {
                                Icon(
                                    if (device.platform == DevicePlatform.Android) Icons.Outlined.PhoneAndroid else Icons.Outlined.Computer,
                                    contentDescription = null,
                                )
                            },
                            headlineContent = { Text(device.name) },
                            supportingContent = {
                                val activity = device.lastActivity?.let {
                                    DateUtils.getRelativeTimeSpanString(it.toEpochMilli(), System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS).toString()
                                }
                                val platform = stringResource(
                                    when (device.platform) {
                                        DevicePlatform.Android -> R.string.platform_android
                                        DevicePlatform.Windows -> R.string.platform_windows
                                        DevicePlatform.Other -> R.string.platform_other
                                    },
                                )
                                val parts = listOfNotNull(
                                    if (device.isThisDevice) stringResource(R.string.devices_this_device) else null,
                                    platform,
                                    activity,
                                    // The space does not vouch for this device (the relay lists it with a certificate that proves
                                    // nothing): shown so that it can be revoked, never announced as a newcomer.
                                    if (!device.verified) stringResource(R.string.devices_unverified) else null,
                                )
                                val inactiveDays = DeviceInactivity.inactiveDays(device, Instant.now())
                                Column {
                                    Text(parts.joinToString(" · "))
                                    if (inactiveDays != null) {
                                        // A hint, never an action taken: the usual confirmation and strong biometrics follow.
                                        val duration = pluralStringResource(R.plurals.devices_inactive_duration, inactiveDays.toInt(), inactiveDays.toInt())
                                        Text(
                                            stringResource(R.string.devices_inactive_hint, duration),
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        )
                                        TextButton(enabled = !busy, onClick = { revoking = device }) { Text(stringResource(R.string.devices_revoke)) }
                                    }
                                }
                            },
                            trailingContent = {
                                androidx.compose.foundation.layout.Row {
                                    IconButton(enabled = !busy, onClick = { renaming = device }) {
                                        Icon(Icons.Outlined.Edit, contentDescription = stringResource(R.string.devices_rename))
                                    }
                                    if (!device.isThisDevice) {
                                        IconButton(enabled = !busy, onClick = { revoking = device }) {
                                            Icon(Icons.Outlined.LinkOff, contentDescription = stringResource(R.string.devices_revoke))
                                        }
                                    }
                                }
                            },
                        )
                        HorizontalDivider()
                    }
                }
            }
        }
    }

    renaming?.let { device ->
        var name by remember(device.id) { mutableStateOf(device.name) }
        AlertDialog(
            onDismissRequest = { renaming = null },
            title = { Text(stringResource(R.string.devices_rename)) },
            text = {
                OutlinedTextField(value = name, onValueChange = { name = it.take(MAX_NAME) }, singleLine = true)
            },
            confirmButton = {
                TextButton(
                    enabled = name.isNotBlank(),
                    onClick = {
                        viewModel.rename(device.id, name)
                        renaming = null
                    },
                ) { Text(stringResource(R.string.action_save)) }
            },
            dismissButton = { TextButton(onClick = { renaming = null }) { Text(stringResource(R.string.action_cancel)) } },
        )
    }

    revoking?.let { device ->
        AlertDialog(
            onDismissRequest = { revoking = null },
            title = { Text(stringResource(R.string.devices_revoke_title, device.name)) },
            text = { Text(stringResource(R.string.devices_revoke_message)) },
            confirmButton = {
                TextButton(onClick = {
                    revoking = null
                    scope.launch {
                        // Destructive action: strong biometrics are required every time.
                        when (authenticator?.forRevocation() ?: AuthOutcome.Unavailable) {
                            AuthOutcome.Success -> viewModel.revoke(device.id)
                            AuthOutcome.Unavailable -> snackbar.showFeedback(revokeUnavailable)
                            AuthOutcome.Cancelled, AuthOutcome.Failed -> Unit
                        }
                    }
                }) { Text(stringResource(R.string.devices_revoke)) }
            },
            dismissButton = { TextButton(onClick = { revoking = null }) { Text(stringResource(R.string.action_cancel)) } },
        )
    }
}

private const val MAX_NAME = 40
