package io.github.darrenn19.nudge

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionConfig
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle

/**
 * Android's "Share your screen?" prompt, from an invisible window (only an
 * activity can show it). Opened by Allow on "wants to see your screen"
 * ([EXTRA_ASK]), or when this phone's user shows their screen on a device
 * ([EXTRA_DEVICE]). On yes, [MirrorService] takes over; on no, the asking
 * device hears no.
 */
class MirrorConsentActivity : Activity() {
    companion object {
        const val EXTRA_ASK = "ask"
        const val EXTRA_DEVICE = "device"
        private const val REQUEST = 1

        /** Show this phone's screen on [device] (a paired device's ID). */
        fun showOn(context: Context, device: String) {
            context.startActivity(
                Intent(context, MirrorConsentActivity::class.java)
                    .putExtra(EXTRA_DEVICE, device)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }

    private var ask = -1L
    private var device: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ask = intent.getLongExtra(EXTRA_ASK, -1)
        device = intent.getStringExtra(EXTRA_DEVICE)
        if (ask >= 0) Notifications.cancelOffer(this, ask)
        if (savedInstanceState != null) return // the prompt is already up
        val manager = getSystemService(MediaProjectionManager::class.java)
        // The whole screen, without asking "one app or entire screen?" first
        // (Android 14+): the PC shows the phone, not one app.
        val prompt = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            manager.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
        } else {
            manager.createScreenCaptureIntent()
        }
        @Suppress("DEPRECATION")
        startActivityForResult(prompt, REQUEST)
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST) return
        if (resultCode == RESULT_OK && data != null) {
            MirrorService.start(this, resultCode, data, ask, device)
        } else if (ask >= 0) {
            Core.answerMirror(ask, false)
        }
        finish()
    }
}
