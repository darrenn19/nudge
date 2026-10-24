package io.github.darrenn19.nudge

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/**
 * Quick settings tile, "Send latest": sends the newest photo or video
 * (a picture just taken, a screenshot, an image or video just downloaded;
 * see [Latest]) to your device. Under the label it names the file it
 * would send.
 *
 * From the lock screen only if the user allowed it (off by default: then
 * Android asks to unlock first). When there's no single obvious device, or
 * Nudge can't read photos yet, it opens the app (after unlocking).
 *
 * Everything is decided during the tap: Android only lets a tile open an
 * activity while handling one. The lookups are quick, and the sending
 * itself happens in the core.
 */
class LatestTileService : TileService() {
    /** Quick settings opened: show what a tap would send. */
    override fun onStartListening() {
        super.onStartListening()
        val tile = qsTile ?: return
        tile.label = "Send latest"
        val latest = if (Latest.allowed(this)) Latest.find(this) else null
        if (Build.VERSION.SDK_INT >= 29) {
            tile.subtitle = latest?.name ?: "Photo or download"
        }
        tile.contentDescription = "Send latest: ${latest?.name ?: "your newest photo or download"}"
        tile.state = Tile.STATE_INACTIVE
        tile.updateTile()
    }

    override fun onClick() {
        if (isLocked && !Latest.whileLocked(this)) {
            unlockAndRun(::send)
        } else {
            send()
        }
    }

    private fun send() {
        if (!Latest.allowed(this)) {
            open(MainActivity.ACTION_ASK_PHOTOS)
            return
        }
        when (Latest.send(this)) {
            Latest.Result.SENT -> {}
            Latest.Result.NONE -> Latest.say(this, "No photo or download to send")
            // A device to choose: in the app, once unlocked.
            Latest.Result.CHOOSE ->
                if (isLocked) unlockAndRun { open(MainActivity.ACTION_SEND_LATEST) }
                else open(MainActivity.ACTION_SEND_LATEST)
        }
    }

    @SuppressLint("StartActivityAndCollapseDeprecated")
    private fun open(action: String) {
        val intent = Intent(this, MainActivity::class.java)
            .setAction(action)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(PendingIntent.getActivity(this, 1, intent, PendingIntent.FLAG_IMMUTABLE))
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }
}
