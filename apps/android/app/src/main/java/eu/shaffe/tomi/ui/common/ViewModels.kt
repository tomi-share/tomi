package eu.shaffe.tomi.ui.common

import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import eu.shaffe.tomi.AppContainer
import eu.shaffe.tomi.TomiApplication

fun android.content.Context.appContainer(): AppContainer =
    (applicationContext as TomiApplication).container

/** Creates a ViewModel that receives the [AppContainer] (manual DI). */
@Composable
inline fun <reified VM : ViewModel> containerViewModel(crossinline create: (AppContainer) -> VM): VM {
    val container = LocalContext.current.appContainer()
    return viewModel(factory = viewModelFactory { initializer { create(container) } })
}
