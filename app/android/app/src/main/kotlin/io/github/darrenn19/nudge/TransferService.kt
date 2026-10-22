package io.github.darrenn19.nudge

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * Keeps the process alive while files are moving, so a transfer started
 * from the share sheet finishes after the user goes back to what they were
 * doing. Shows the progress notification; does no work itself (the Rust
 * core does the transfer).
 */
class TransferService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val state = latest
        if (state == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        running = true
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
        } else {
            0
        }
        ServiceCompat.startForeground(this, Notifications.PROGRESS_ID, state.notification(this), type)
        return START_NOT_STICKY
    }

    /** Android 15 limits how long data-sync services run; stop cleanly when told. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        stopSelf()
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }

    private data class State(val title: String, val text: String, val progress: Int) {
        fun notification(context: Context) = Notifications.progress(context, title, text, progress)
    }

    companion object {
        @Volatile private var latest: State? = null
        @Volatile private var running = false

        /** Show or refresh the progress notification, starting the service if needed. */
        fun update(context: Context, title: String, text: String, progress: Int) {
            val state = State(title, text, progress)
            latest = state
            if (running) {
                Notifications.notify(context, Notifications.PROGRESS_ID, state.notification(context))
                return
            }
            try {
                ContextCompat.startForegroundService(context, Intent(context, TransferService::class.java))
            } catch (e: Exception) {
                // Android 12+ won't start one from the background (say, a file
                // arriving while Nudge isn't open). Show progress anyway.
                Notifications.notify(context, Notifications.PROGRESS_ID, state.notification(context))
            }
        }

        fun stop(context: Context) {
            latest = null
            context.stopService(Intent(context, TransferService::class.java))
            Notifications.cancel(context, Notifications.PROGRESS_ID)
        }
    }
}
