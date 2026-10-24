package io.github.darrenn19.nudge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Accept or Decline on an "incoming files" or "wants to pair" notification. */
class OfferActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getLongExtra(EXTRA_ID, -1)
        if (id < 0) return
        val accept = intent.getBooleanExtra(EXTRA_ACCEPT, false)
        Notifications.cancelOffer(context, id)
        if (intent.getBooleanExtra(EXTRA_PAIRING, false)) {
            Core.answerPair(id, accept)
        } else {
            Core.answerOffer(id, accept)
        }
    }

    companion object {
        const val EXTRA_ID = "id"
        const val EXTRA_ACCEPT = "accept"
        const val EXTRA_PAIRING = "pairing"
    }
}
