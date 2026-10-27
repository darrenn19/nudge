package io.github.darrenn19.nudge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Cancel on the "Sending…" / "Receiving…" notification: stop those transfers. */
class CancelReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        intent.getLongArrayExtra(EXTRA_IDS)?.forEach { Core.cancel(it) }
    }

    companion object {
        const val EXTRA_IDS = "ids"
    }
}
