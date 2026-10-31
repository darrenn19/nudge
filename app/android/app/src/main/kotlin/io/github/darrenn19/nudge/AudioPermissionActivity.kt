package io.github.darrenn19.nudge

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle

/**
 * Asks for RECORD_AUDIO, which Android requires to capture what apps play,
 * the first time a PC asks for this phone's sound during mirroring (from
 * the "Play this phone's sound on <PC>?" notification). No window of its
 * own: just Android's question. Then the sound starts.
 */
class AudioPermissionActivity : Activity() {
    companion object {
        private const val REQUEST = 1

        fun intent(context: Context): Intent =
            Intent(context, AudioPermissionActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Notifications.cancel(this, Notifications.AUDIO_SETUP_ID)
        if (savedInstanceState != null) return
        requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQUEST)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) MirrorService.audioAllowed()
        finish()
    }
}
