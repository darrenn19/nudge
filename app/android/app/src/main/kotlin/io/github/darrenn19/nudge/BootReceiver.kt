package io.github.darrenn19.nudge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Starts Nudge after the phone restarts or the app updates, so files can
 * arrive before the user opens it.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            // After the first unlock: Nudge's identity lives in storage
            // that's encrypted until then.
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED,
            -> NudgeService.start(context)
        }
    }
}
