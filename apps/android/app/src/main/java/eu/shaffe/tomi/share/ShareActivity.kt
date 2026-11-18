package eu.shaffe.tomi.share

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.lifecycle.lifecycleScope
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.MainActivity
import eu.shaffe.tomi.TomiApplication
import eu.shaffe.tomi.R
import eu.shaffe.tomi.ui.common.FlagSecureEffect
import eu.shaffe.tomi.ui.send.SendOptionsSheet
import eu.shaffe.tomi.ui.send.SentBanner
import eu.shaffe.tomi.ui.theme.TomiTheme
import kotlinx.coroutines.launch

/**
 * Share target for ACTION_SEND / ACTION_SEND_MULTIPLE. A translucent activity on top of the source app:
 * the share is sent immediately with default settings, then a 3 s banner offers Undo and Options.
 * Content that looks like a secret opens a confirmation sheet instead.
 */
class ShareActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // The core opens off the main thread (CoreStartup): wait for it. When it is already open this runs straight through.
        lifecycleScope.launch {
            val container = (application as TomiApplication).awaitContainer()
            if (container == null) {
                setContent { TomiTheme { UnreadableShare(onFinish = ::finish, onOpenApp = ::openApp) } }
            } else {
                begin(container, savedInstanceState)
            }
        }
    }

    private fun begin(container: AppContainer, savedInstanceState: Bundle?) {
        val viewModel = androidx.lifecycle.ViewModelProvider(
            this,
            viewModelFactory { initializer { ShareViewModel(container) } },
        )[ShareViewModel::class.java]

        // Restored after process death: the original share was already handled, never send it twice.
        if (savedInstanceState != null && !viewModel.hasStarted) {
            finish()
            return
        }
        if (!viewModel.hasStarted) {
            val raw = ShareIntentReader.extract(this, intent)
            val appContext = applicationContext
            // Provider queries (name, size, type of each file) run on the IO dispatcher, not in onCreate.
            viewModel.start { ShareIntentReader.resolve(appContext, raw, container.ownAuthorities) }
            SharingShortcut.reportUsed(this)
        }

        setContent {
            TomiTheme {
                ShareScreen(viewModel, onFinish = ::finish, onOpenApp = ::openApp)
            }
        }
    }

    private fun openApp() {
        startActivity(Intent(this, MainActivity::class.java))
        finish()
    }
}

/** The core cannot be opened (damaged or lost data): the share cannot be sent; the app offers the recovery. */
@Composable
private fun UnreadableShare(onFinish: () -> Unit, onOpenApp: () -> Unit) {
    Box(Modifier.fillMaxSize().safeDrawingPadding(), contentAlignment = Alignment.BottomCenter) {
        MessageCard(
            message = stringResource(R.string.share_unreadable),
            actionLabel = stringResource(R.string.share_open_app),
            onAction = onOpenApp,
            onDismiss = onFinish,
        )
    }
}

/** Edge-to-edge: the banner and message cards never sit under the gesture bar or a display cutout (see ui/common/Insets.kt). */
@Composable
private fun ShareScreen(viewModel: ShareViewModel, onFinish: () -> Unit, onOpenApp: () -> Unit) {
    Box(Modifier.fillMaxSize().safeDrawingPadding()) {
        ShareContent(viewModel, onFinish, onOpenApp)
    }
}

@Composable
private fun ShareContent(viewModel: ShareViewModel, onFinish: () -> Unit, onOpenApp: () -> Unit) {
    val state by viewModel.state.collectAsStateWithLifecycle()

    val showsSecret = when (val s = state) {
        is ShareUiState.Confirm -> s.secret
        is ShareUiState.Options -> s.secret
        else -> false
    }
    FlagSecureEffect(enabled = showsSecret)

    when (val s = state) {
        ShareUiState.Loading -> Unit

        ShareUiState.NothingToShare -> LaunchedEffect(Unit) { onFinish() }

        ShareUiState.NotPaired -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            MessageCard(
                message = stringResource(R.string.share_not_paired),
                actionLabel = stringResource(R.string.share_open_app),
                onAction = onOpenApp,
                onDismiss = onFinish,
            )
        }

        ShareUiState.TooLate -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            MessageCard(
                message = stringResource(R.string.share_too_late),
                actionLabel = null,
                onAction = {},
                onDismiss = onFinish,
            )
        }

        ShareUiState.Failed -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            MessageCard(
                message = stringResource(R.string.share_failed),
                actionLabel = null,
                onAction = {},
                onDismiss = onFinish,
            )
        }

        is ShareUiState.Sent -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            SentBanner(
                skippedFiles = s.submission.skippedFiles,
                deadline = s.submission.deadline,
                onUndo = { viewModel.undo(onFinish) },
                onOptions = viewModel::openOptions,
                onTimeout = onFinish,
                modifier = Modifier.padding(16.dp).navigationBarsPadding(),
            )
        }

        is ShareUiState.Confirm -> SendOptionsSheet(
            preview = previewText(s.parsed, masked = s.secret),
            secret = s.secret,
            hasAttachment = s.parsed.streams.isNotEmpty(),
            secretLocked = s.secretLocked,
            expiry = s.expiry,
            infoRes = R.string.share_secret_detected,
            confirmLabelRes = R.string.action_send,
            cancelLabelRes = R.string.action_cancel,
            onSecretChange = viewModel::onSecretChange,
            onExpiryChange = viewModel::onExpiryChange,
            onConfirm = viewModel::confirm,
            onCancel = onFinish,
        )

        is ShareUiState.Options -> SendOptionsSheet(
            preview = previewText(s.parsed, masked = s.secret),
            secret = s.secret,
            hasAttachment = s.parsed.streams.isNotEmpty(),
            secretLocked = false,
            expiry = s.expiry,
            infoRes = null,
            confirmLabelRes = R.string.action_send,
            cancelLabelRes = R.string.action_cancel_send,
            onSecretChange = viewModel::onSecretChange,
            onExpiryChange = viewModel::onExpiryChange,
            onConfirm = viewModel::confirm,
            onCancel = onFinish,
        )
    }
}

@Composable
private fun MessageCard(message: String, actionLabel: String?, onAction: () -> Unit, onDismiss: () -> Unit) {
    Surface(
        modifier = Modifier.fillMaxWidth().padding(16.dp).navigationBarsPadding(),
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.inverseSurface,
        contentColor = MaterialTheme.colorScheme.inverseOnSurface,
        shadowElevation = 6.dp,
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(message, style = MaterialTheme.typography.bodyLarge)
            if (actionLabel != null) {
                Button(onClick = onAction) { Text(actionLabel) }
            }
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_close), color = MaterialTheme.colorScheme.inversePrimary) }
        }
    }
}

/** Preview line of the sheet: the content of a secret is never shown. */
@Composable
private fun previewText(parsed: ParsedShare, masked: Boolean): String {
    if (masked) return stringResource(R.string.preview_secret_masked)
    val parts = listOfNotNull(
        parsed.title,
        parsed.url,
        parsed.text?.plain?.take(PREVIEW_CHARS),
        parsed.streams.takeIf { it.isNotEmpty() }?.let { pluralStringResource(R.plurals.preview_files, it.size, it.size) },
    )
    return parts.joinToString("\n")
}

private const val PREVIEW_CHARS = 160
