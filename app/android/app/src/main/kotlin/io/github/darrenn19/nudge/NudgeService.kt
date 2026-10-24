package io.github.darrenn19.nudge

import android.annotation.SuppressLint
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * Keeps Nudge running so files arrive without the app open: the Rust core
 * listens for paired devices, and each arrival shows a notification.
 * Android only lets an app stay running in the background with an ongoing
 * notification; this one is quiet ("Ready to receive", minimised) and turns
 * into the progress bar while files move.
 *
 * Started when the app opens and at boot ([BootReceiver]). Does no transfer
 * work itself.
 */
class NudgeService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
        } else {
            0
        }
        try {
            ServiceCompat.startForeground(this, Notifications.SERVICE_ID, current(this), type)
            running = true
            ClipWatch.serviceStarted(this)
        } catch (e: Exception) {
            // Android refused (started from the background without the
            // battery exemption). Try again next time the app opens.
            stopSelf()
            return START_NOT_STICKY
        }
        // If Android stops the process to free memory, bring it back.
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        releaseLocks()
        super.onDestroy()
    }

    private data class Progress(val title: String, val text: String, val percent: Int)

    companion object {
        @Volatile private var progress: Progress? = null
        @Volatile private var running = false
        private var wakeLock: PowerManager.WakeLock? = null
        private var wifiLock: WifiManager.WifiLock? = null
        private var lowLatencyLock: WifiManager.WifiLock? = null

        /** Start (or keep) running. Safe to call any time. */
        fun start(context: Context) {
            if (running) return
            try {
                ContextCompat.startForegroundService(context, Intent(context, NudgeService::class.java))
            } catch (e: Exception) {
                // Not allowed from the background right now; the app tries
                // again when it opens.
            }
        }

        /**
         * Files are moving (or not): keep the CPU awake and Wi-Fi out of
         * power saving for the whole transfer, on screen or off. Power-save
         * Wi-Fi alone can cut throughput several times over.
         */
        fun holdNetwork(context: Context, on: Boolean) {
            if (on) holdLocks(context) else releaseLocks()
        }

        /** Files are moving: show progress, and keep the CPU and Wi-Fi awake. */
        fun showProgress(context: Context, title: String, text: String, percent: Int) {
            progress = Progress(title, text, percent)
            holdLocks(context)
            if (running) {
                Notifications.notify(context, Notifications.SERVICE_ID, current(context))
            } else {
                start(context)
                // Shown even if the service couldn't start.
                Notifications.notify(context, Notifications.SERVICE_ID, current(context))
            }
        }

        /** Nothing moving: back to "Ready to receive". */
        fun idle(context: Context) {
            progress = null
            if (running) {
                Notifications.notify(context, Notifications.SERVICE_ID, current(context))
            } else {
                Notifications.cancel(context, Notifications.SERVICE_ID)
            }
        }

        private fun current(context: Context) = progress?.let {
            Notifications.progress(context, it.title, it.text, it.percent)
        } ?: Notifications.ready(context)

        /**
         * With the screen off, the CPU sleeps and Wi-Fi drops to power-save,
         * which would stall a transfer. Held only while files move, with a
         * timeout in case the "idle" call never comes.
         */
        @SuppressLint("WakelockTimeout")
        @Synchronized
        private fun holdLocks(context: Context) {
            val app = context.applicationContext
            val wake = wakeLock ?: app.getSystemService(PowerManager::class.java)
                ?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "nudge:transfer")
                ?.apply { setReferenceCounted(false) }
                ?.also { wakeLock = it }
            // Re-acquiring pushes the timeout back while progress keeps coming.
            wake?.acquire(LOCK_TIMEOUT_MS)
            val wifi = wifiLock ?: (app.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
                ?.let {
                    @Suppress("DEPRECATION") // The only mode that also works with the screen off.
                    it.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "nudge:transfer")
                }
                ?.apply { setReferenceCounted(false) }
                ?.also { wifiLock = it }
            if (wifi?.isHeld == false) wifi.acquire()
            // Android 10+: the lowest-latency mode, which also stops power
            // saving; it applies while Nudge is on screen.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val fast = lowLatencyLock ?: (app.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
                    ?.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "nudge:fast")
                    ?.apply { setReferenceCounted(false) }
                    ?.also { lowLatencyLock = it }
                if (fast?.isHeld == false) fast.acquire()
            }
        }

        @Synchronized
        private fun releaseLocks() {
            wakeLock?.takeIf { it.isHeld }?.release()
            wifiLock?.takeIf { it.isHeld }?.release()
            lowLatencyLock?.takeIf { it.isHeld }?.release()
        }

        private const val LOCK_TIMEOUT_MS = 10 * 60 * 1000L
    }
}
