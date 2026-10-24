package io.github.darrenn19.nudge

import android.app.Activity
import android.os.Handler
import android.os.Looper
import android.widget.Toast

/**
 * "Send clipboard" (the notification's button):
 * puts the phone's clipboard on your other devices, in one tap.
 *
 * Android only lets the app in front read the clipboard, so this is an
 * invisible window: it reads as soon as it has focus, sends, and closes.
 */
class ClipActivity : Activity() {
    private var done = false

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        done = true
        val item = ClipSync.copiedItem(this, again = true)
        if (item == null) {
            say("Nothing to send: the clipboard is empty")
            finish()
            return
        }
        val app = applicationContext
        Thread {
            val names = ClipSync.send(app, item)
            say(
                if (names.isEmpty()) "Couldn't reach a device that shares the clipboard"
                else "Copied to ${names.joinToString(", ")}",
            )
        }.start()
        // Read; sending carries on without the window.
        finish()
    }

    private fun say(text: String) = Handler(Looper.getMainLooper()).post {
        Toast.makeText(applicationContext, text, Toast.LENGTH_SHORT).show()
    }
}
