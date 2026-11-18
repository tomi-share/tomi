package eu.shaffe.tomi.tile

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import eu.shaffe.tomi.MainActivity

/**
 * Quick Settings tile "Send": opens the compose screen. It never touches the clipboard;
 * the user pastes with the visible button of that screen.
 */
class SendTileService : TileService() {

    override fun onStartListening() {
        qsTile?.apply {
            state = Tile.STATE_INACTIVE
            updateTile()
        }
    }

    // The Intent overload is only used below API 34 (minSdk is 33); API 34+ takes the PendingIntent branch.
    @SuppressLint("StartActivityAndCollapseDeprecated")
    override fun onClick() {
        val intent = Intent(this, MainActivity::class.java)
            .setData(Uri.parse("tomi-share://send"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val pending = PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            startActivityAndCollapse(pending)
        } else {
            @Suppress("DEPRECATION", "StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(intent)
        }
    }
}
