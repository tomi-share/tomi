package eu.shaffe.tomi.ui.feed

import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import eu.shaffe.tomi.R
import eu.shaffe.tomi.core.RelayRevocationVia

/**
 * Shown for as long as the core says the removal of a device is not finished (`TomiCore.rotationStalled`): the key rotation that must
 * follow keeps failing and the core retries it every hour, so the removed device may still hold the current key. Persistent but calm: no
 * button (nothing to do but wait, or open Tomi on the other devices), no alarm colour, no icon. The caller shows it only while it is true.
 */
@Composable
fun RotationStalledBanner(modifier: Modifier = Modifier) {
    BannerCard(
        title = stringResource(R.string.rotation_stalled_title),
        text = stringResource(R.string.rotation_stalled_message),
        progress = false,
        modifier = modifier,
    )
}

/** The wording of the one-time notice for a device that the relay alone removed. */
object RelayRevocationNotice {
    /** The message, with the device's name as its only argument. */
    @StringRes
    fun messageRes(via: RelayRevocationVia): Int = when (via) {
        RelayRevocationVia.Admin -> R.string.snackbar_relay_revoked_admin
        RelayRevocationVia.Self -> R.string.snackbar_relay_revoked_self
        RelayRevocationVia.Other -> R.string.snackbar_relay_revoked_other
    }
}
