package eu.shaffe.tomi

import android.content.Intent
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.lifecycleScope
import eu.shaffe.tomi.deeplink.DeepLink
import eu.shaffe.tomi.deeplink.DeepLinkParser
import eu.shaffe.tomi.ui.StartupGate
import eu.shaffe.tomi.ui.TomiRoot
import eu.shaffe.tomi.ui.theme.TomiTheme
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** Holds the deep link received by the activity until the navigation graph has handled it. */
class MainViewModel : ViewModel() {
    private val mutableLink = MutableStateFlow<DeepLink?>(null)
    val link: StateFlow<DeepLink?> = mutableLink.asStateFlow()

    /** Only parses; the parser is strict and every command only pre-fills or navigates. */
    fun onIntent(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        DeepLinkParser.parse(intent.dataString)?.let { mutableLink.value = it }
    }

    fun consume() {
        mutableLink.value = null
    }
}

/** FragmentActivity because BiometricPrompt needs one. */
class MainActivity : FragmentActivity() {

    private lateinit var mainViewModel: MainViewModel

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        mainViewModel = ViewModelProvider(this)[MainViewModel::class.java]
        // On recreation (rotation) the same intent is delivered again: it must not be replayed.
        if (savedInstanceState == null) mainViewModel.onIntent(intent)
        // The focus event of a cold start usually comes while the core is still opening (no container yet, see onWindowFocusChanged): once
        // the core is ready the current focus is replayed, so that the clipboard left after a departure is cleared and an overdue secret
        // is checked even if the activity never gets another focus event.
        lifecycleScope.launch {
            (application as TomiApplication).startup.state.collect { state ->
                if (state is StartupState.Ready) state.value.onWindowFocusChanged(hasWindowFocus())
            }
        }
        setContent {
            TomiTheme {
                StartupGate((application as TomiApplication).startup) { TomiRoot(mainViewModel, onLeftSpace = ::restartFromWelcome) }
            }
        }
    }

    /**
     * The device left its space: a new task. The view models of this one (secrets shown in clear, drafts, the state of the pairing) are
     * destroyed with it, and the new one starts on the welcome screen. Back cannot return to the old feed. The intent has no VIEW action,
     * so [MainViewModel.onIntent] ignores it.
     */
    private fun restartFromWelcome() {
        startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // The clipboard metadata is only readable while we have focus: an overdue secret, or a clip left by a departure, is checked then.
        // Not there while the core is opening or unreadable.
        (application as TomiApplication).containerOrNull?.onWindowFocusChanged(hasFocus)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        mainViewModel.onIntent(intent)
    }
}
