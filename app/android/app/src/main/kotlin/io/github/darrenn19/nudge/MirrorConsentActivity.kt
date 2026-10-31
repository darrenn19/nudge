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
        const val EXTRA_RESUME = "resume"
        private const val REQUEST = 1

        /**
         * Says yes to a paired device's request [ask] straight away (no
         * question of Nudge's own). False if Android won't let Nudge open
         * a screen from the background (no "display over other apps").
         */
        fun answerYes(context: Context, ask: Long): Boolean {
            if (!android.provider.Settings.canDrawOverlays(context)) return false
            return runCatching {
                context.startActivity(
                    Intent(context, MirrorConsentActivity::class.java)
                        .putExtra(EXTRA_ASK, ask)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            }.isSuccess
        }

        /**
         * A request [ask] that came while the phone was locked: says yes as
         * soon as it's unlocked (the request stays open for a minute), so
         * nothing has to be tapped (the notification stays if it can't).
         */
        fun answerOnUnlock(context: Context, ask: Long) {
            val app = context.applicationContext
            val main = android.os.Handler(android.os.Looper.getMainLooper())
            var done = false
            lateinit var receiver: android.content.BroadcastReceiver
            fun finish() {
                if (done) return
                done = true
                runCatching { app.unregisterReceiver(receiver) }
            }
            fun tryNow() {
                if (done || LockState.isLocked(app)) return
                finish()
                if (answerYes(app, ask)) {
                    Notifications.cancelOffer(app, ask)
                    android.util.Log.i("Nudge", "mirroring: unlocked; sharing as asked while locked")
                }
            }
            receiver = object : android.content.BroadcastReceiver() {
                override fun onReceive(c: Context, intent: Intent) {
                    // "User present", or a screen on with no lock screen to pass.
                    if (intent.action == Intent.ACTION_USER_PRESENT) tryNow() else main.postDelayed({ tryNow() }, 1_000)
                }
            }
            androidx.core.content.ContextCompat.registerReceiver(
                app, receiver,
                android.content.IntentFilter().apply {
                    addAction(Intent.ACTION_USER_PRESENT)
                    addAction(Intent.ACTION_SCREEN_ON)
                },
                androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED,
            )
            // The request lapses after a minute (the core's ASK_TIMEOUT).
            main.postDelayed({ finish() }, 60_000)
        }

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
    private var resume = false
    private var device: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ask = intent.getLongExtra(EXTRA_ASK, -1)
        device = intent.getStringExtra(EXTRA_DEVICE)
        resume = intent.getBooleanExtra(EXTRA_RESUME, false)
        if (resume) Notifications.cancel(this, Notifications.RESUME_ID)
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
        if (resume) {
            // Android stopped the capture at the lock: the running session carries on with this one.
            MirrorService.resume(resultCode, data)
        } else if (resultCode == RESULT_OK && data != null) {
            MirrorService.start(this, resultCode, data, ask, device)
        } else if (ask >= 0) {
            Core.answerMirror(ask, false)
        }
        finish()
    }
}
