package io.github.darrenn19.nudge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Accept or Decline on an "incoming files", "wants to pair" or "wants to see
 * your screen" notification, and Stop on "your screen is being shared".
 */
class OfferActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getLongExtra(EXTRA_ID, -1)
        if (id < 0) return
        if (intent.getBooleanExtra(EXTRA_STOP_MIRROR, false)) {
            MirrorService.stop(context)
            return
        }
        val accept = intent.getBooleanExtra(EXTRA_ACCEPT, false)
        Notifications.cancelOffer(context, id)
        when {
            intent.getBooleanExtra(EXTRA_PAIRING, false) -> Core.answerPair(id, accept)
            intent.getBooleanExtra(EXTRA_MIRROR, false) -> Core.answerMirror(id, accept)
            else -> Core.answerOffer(id, accept)
        }
    }

    companion object {
        const val EXTRA_ID = "id"
        const val EXTRA_ACCEPT = "accept"
        const val EXTRA_PAIRING = "pairing"
        const val EXTRA_MIRROR = "mirror"
        const val EXTRA_STOP_MIRROR = "stopMirror"
    }
}
