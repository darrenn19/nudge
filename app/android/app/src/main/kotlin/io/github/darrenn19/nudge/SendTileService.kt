package io.github.darrenn19.nudge

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.TileService

/** Quick settings tile: tap it, pick files, they're sent. */
class SendTileService : TileService() {
    override fun onClick() {
        if (isLocked) unlockAndRun(::openPicker) else openPicker()
    }

    @SuppressLint("StartActivityAndCollapseDeprecated")
    private fun openPicker() {
        val intent = Intent(this, MainActivity::class.java)
            .setAction(MainActivity.ACTION_PICK_FILES)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE))
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }
}
