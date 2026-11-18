package eu.shaffe.tomi.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import androidx.compose.ui.platform.LocalContext
import eu.shaffe.tomi.MainViewModel
import eu.shaffe.tomi.deeplink.DeepLink
import eu.shaffe.tomi.ui.common.FlagSecureEffect
import eu.shaffe.tomi.ui.common.appContainer
import eu.shaffe.tomi.ui.common.containerViewModel
import eu.shaffe.tomi.ui.code.CodeFileScreen
import eu.shaffe.tomi.ui.devices.DevicesScreen
import eu.shaffe.tomi.ui.devices.DevicesViewModel
import eu.shaffe.tomi.ui.feed.FeedScreen
import eu.shaffe.tomi.ui.feed.FeedViewModel
import eu.shaffe.tomi.ui.feed.ItemDetailScreen
import eu.shaffe.tomi.ui.onboarding.OnboardingScreen
import eu.shaffe.tomi.ui.pairing.CreateSpaceScreen
import eu.shaffe.tomi.ui.pairing.CreateSpaceViewModel
import eu.shaffe.tomi.ui.pairing.InviteScreen
import eu.shaffe.tomi.ui.pairing.InviteViewModel
import eu.shaffe.tomi.ui.pairing.PairingScreen
import eu.shaffe.tomi.ui.pairing.PairingViewModel
import eu.shaffe.tomi.ui.pairing.WelcomeScreen
import eu.shaffe.tomi.ui.send.SendScreen
import eu.shaffe.tomi.ui.send.SendViewModel
import eu.shaffe.tomi.ui.viewer.ImageViewerScreen

private object Routes {
    /** First screen of a device that is not in a space yet: create the space or join it. */
    const val WELCOME = "welcome"

    /** Joins an existing space from another device: QR code or invitation code, comparison of the verification code. */
    const val PAIR = "pair"

    /** First device of a space: creates it with the installation token of the relay. */
    const val CREATE = "create"

    /** Adds a device to the space from this one: code, QR code, comparison of the verification code. */
    const val INVITE = "invite"
    const val ONBOARDING = "onboarding"
    const val FEED = "feed"
    const val SEND = "send"
    const val DEVICES = "devices"
    const val ITEM = "item/{id}"
    fun item(id: String) = "item/$id"

    /** Internal full-screen image viewer: item id and SLOT (`Attachment.slot`) of the first image to show. */
    const val VIEWER = "viewer/{id}/{slot}"
    fun viewer(id: String, slot: Int) = "viewer/$id/$slot"

    /** Internal full-screen code screen: item id and SLOT (`Attachment.slot`) of the first code file to show. */
    const val CODE = "code/{id}/{slot}"
    fun code(id: String, slot: Int) = "code/$id/$slot"
}

@Composable
fun TomiRoot(mainViewModel: MainViewModel, onLeftSpace: () -> Unit) {
    val container = LocalContext.current.appContainer()
    val nav = rememberNavController()
    val feedViewModel = containerViewModel { FeedViewModel(it) }
    val sendViewModel = containerViewModel { SendViewModel(it) }
    val pairingViewModel = containerViewModel { PairingViewModel(it) }
    val devicesViewModel = containerViewModel { DevicesViewModel(it.core) }
    val paired by container.core.isPaired.collectAsStateWithLifecycle()
    // Left the space (Feed menu, or a departure settled at start-up): every screen starts again from the welcome screen, by a new task.
    // Saveable: a recreation of the activity (rotation, process death with saved state) must still see that the device was paired when
    // the restored back stack is the feed; otherwise a departure that ends meanwhile would leave an empty feed on an unpaired device.
    var wasPaired by rememberSaveable { mutableStateOf(paired) }
    LaunchedEffect(paired) {
        if (wasPaired && !paired) onLeftSpace()
        wasPaired = paired
    }
    val feedState by feedViewModel.uiState.collectAsStateWithLifecycle()
    // A creation link waiting for the "create your space" screen to take it. Never submitted from here.
    var pendingSetup by remember { mutableStateOf<DeepLink.Setup?>(null) }
    val startDestination = remember { if (container.core.isPaired.value) Routes.FEED else Routes.WELCOME }

    // No screenshot or recording while any secret is shown in clear, and secrets are hidden again when the app leaves the screen.
    FlagSecureEffect(enabled = feedState.revealedIds.isNotEmpty())
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { feedViewModel.hideAll() }

    // The device is in a space (joined or created): onboarding the first time, then the feed. Leaves the pairing screens for good.
    val onSpaceReady = {
        val next = if (container.prefs.onboardingDone) Routes.FEED else Routes.ONBOARDING
        nav.navigate(next) { popUpTo(Routes.WELCOME) { inclusive = true } }
        Unit
    }

    NavHost(navController = nav, startDestination = startDestination) {
        composable(Routes.WELCOME) {
            WelcomeScreen(
                onCreateSpace = { nav.navigate(Routes.CREATE) { launchSingleTop = true } },
                onJoinSpace = { nav.navigate(Routes.PAIR) { launchSingleTop = true } },
            )
        }
        composable(Routes.PAIR) {
            PairingScreen(
                pairingViewModel,
                onFinished = onSpaceReady,
                onBack = { nav.popBackStack() },
                // Straight to the demo feed, without the first-launch screen (like iOS).
                onDemoEntered = { nav.navigate(Routes.FEED) { popUpTo(Routes.WELCOME) { inclusive = true } } },
            )
        }
        composable(Routes.CREATE) {
            // Scoped to this destination: the token typed here is gone when the screen is left.
            val createSpaceViewModel = containerViewModel { CreateSpaceViewModel(it) }
            // A creation link (App Link): fills the relay and the key, submits nothing.
            LaunchedEffect(pendingSetup) {
                pendingSetup?.let {
                    createSpaceViewModel.onSetupLink(it.relayUrl, it.token)
                    pendingSetup = null
                }
            }
            CreateSpaceScreen(
                createSpaceViewModel,
                onBack = { nav.popBackStack() },
                // The relay already has a space: on to "join a space".
                onJoin = { nav.navigate(Routes.PAIR) { launchSingleTop = true } },
                onFinished = onSpaceReady,
            )
        }
        composable(Routes.INVITE) {
            // Scoped to this destination: leaving the screen, by any way, closes the invitation at the relay.
            val inviteViewModel = containerViewModel { InviteViewModel(it) }
            InviteScreen(inviteViewModel, onBack = { nav.popBackStack() })
        }
        composable(Routes.ONBOARDING) {
            OnboardingScreen(
                // Opened from the menu of the feed (something is below it) rather than right after the pairing.
                fromHelp = nav.previousBackStackEntry != null,
                onDone = {
                    container.prefs.onboardingDone = true
                    if (nav.previousBackStackEntry != null) {
                        nav.popBackStack()
                    } else {
                        nav.navigate(Routes.FEED) { popUpTo(Routes.ONBOARDING) { inclusive = true } }
                    }
                },
            )
        }
        composable(Routes.FEED) {
            FeedScreen(
                viewModel = feedViewModel,
                onSend = { nav.navigate(Routes.SEND) { launchSingleTop = true } },
                onDevices = { nav.navigate(Routes.DEVICES) { launchSingleTop = true } },
                onShareHelp = { nav.navigate(Routes.ONBOARDING) { launchSingleTop = true } },
                onOpenItem = { nav.navigate(Routes.item(it)) { launchSingleTop = true } },
                onOpenImage = { id, slot -> nav.navigate(Routes.viewer(id, slot)) { launchSingleTop = true } },
                onOpenCodeFile = { id, slot -> nav.navigate(Routes.code(id, slot)) { launchSingleTop = true } },
                sentSubmission = sendViewModel.submission,
                onUndoSend = {
                    // The compose screen shows the text back once the core has cancelled, or says "too late".
                    sendViewModel.undo()
                    nav.navigate(Routes.SEND) { launchSingleTop = true }
                },
                onSentBannerTimeout = sendViewModel::onBannerTimeout,
            )
        }
        composable(Routes.SEND) {
            SendScreen(sendViewModel, onBack = { nav.popBackStack() })
        }
        composable(Routes.DEVICES) {
            DevicesScreen(
                devicesViewModel,
                onBack = { nav.popBackStack() },
                onAddDevice = { nav.navigate(Routes.INVITE) { launchSingleTop = true } },
            )
        }
        composable(Routes.ITEM, arguments = listOf(navArgument("id") { type = NavType.StringType })) { entry ->
            val itemId = entry.arguments?.getString("id").orEmpty()
            ItemDetailScreen(
                itemId = itemId,
                viewModel = feedViewModel,
                onBack = { nav.popBackStack() },
                onOpenImage = { slot -> nav.navigate(Routes.viewer(itemId, slot)) { launchSingleTop = true } },
                onOpenCodeFile = { slot -> nav.navigate(Routes.code(itemId, slot)) { launchSingleTop = true } },
            )
        }
        composable(
            Routes.CODE,
            arguments = listOf(
                navArgument("id") { type = NavType.StringType },
                navArgument("slot") { type = NavType.IntType },
            ),
        ) { entry ->
            val itemId = entry.arguments?.getString("id").orEmpty()
            CodeFileScreen(
                itemId = itemId,
                startSlot = entry.arguments?.getInt("slot") ?: 0,
                viewModel = feedViewModel,
                onBack = { nav.popBackStack() },
                onOpenDetail = {
                    // Coming from the detail of the same item: go back to it instead of stacking a second one.
                    val below = nav.previousBackStackEntry
                    if (below?.destination?.route == Routes.ITEM && below.arguments?.getString("id") == itemId) {
                        nav.popBackStack()
                    } else {
                        nav.navigate(Routes.item(itemId)) {
                            popUpTo(Routes.CODE) { inclusive = true }
                            launchSingleTop = true
                        }
                    }
                },
            )
        }
        composable(
            Routes.VIEWER,
            arguments = listOf(
                navArgument("id") { type = NavType.StringType },
                navArgument("slot") { type = NavType.IntType },
            ),
        ) { entry ->
            val itemId = entry.arguments?.getString("id").orEmpty()
            ImageViewerScreen(
                itemId = itemId,
                startSlot = entry.arguments?.getInt("slot") ?: 0,
                viewModel = feedViewModel,
                onBack = { nav.popBackStack() },
                onOpenDetail = {
                    // Coming from the detail of the same item: go back to it instead of stacking a second one.
                    val below = nav.previousBackStackEntry
                    if (below?.destination?.route == Routes.ITEM && below.arguments?.getString("id") == itemId) {
                        nav.popBackStack()
                    } else {
                        nav.navigate(Routes.item(itemId)) {
                            popUpTo(Routes.VIEWER) { inclusive = true }
                            launchSingleTop = true
                        }
                    }
                },
            )
        }
    }

    // Deep links and notification taps. They only navigate or pre-fill; nothing is ever sent from here.
    val link by mainViewModel.link.collectAsStateWithLifecycle()
    LaunchedEffect(link, paired) {
        val pending = link ?: return@LaunchedEffect
        when {
            pending is DeepLink.Pair -> if (!paired) {
                pairingViewModel.offerPayload(pending.payload, pending.relayUrl)
                nav.navigate(Routes.PAIR) { launchSingleTop = true }
            }
            pending is DeepLink.Setup -> if (!paired) {
                pendingSetup = pending
                nav.navigate(Routes.CREATE) { launchSingleTop = true }
            }
            !paired -> Unit
            else -> when (pending) {
                DeepLink.Open -> nav.navigate(Routes.FEED) { popUpTo(Routes.FEED); launchSingleTop = true }
                is DeepLink.Send -> {
                    sendViewModel.prefill(pending)
                    nav.navigate(Routes.SEND) { launchSingleTop = true }
                }
                is DeepLink.Item -> nav.navigate(Routes.item(pending.id)) { launchSingleTop = true }
                is DeepLink.Pair -> Unit
                is DeepLink.Setup -> Unit
            }
        }
        mainViewModel.consume()
    }
}
