package io.github.darrenn19.nudge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Accept or Decline on an "incoming files" notification: pass it to Dart. */
class OfferActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getLongExtra(EXTRA_ID, -1)
        if (id < 0) return
        val accept = intent.getBooleanExtra(EXTRA_ACCEPT, false)
        Notifications.cancelOffer(context, id)
        NudgeApplication.platform?.invokeMethod("offerAnswer", mapOf("id" to id, "accept" to accept))
    }

    companion object {
        const val EXTRA_ID = "id"
        const val EXTRA_ACCEPT = "accept"
    }
}
