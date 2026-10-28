package io.github.darrenn19.nudge

import android.app.KeyguardManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.PowerManager
import androidx.core.content.ContextCompat
import java.io.File

/**
 * The "Accept files while locked" setting, applied as the phone locks and
 * unlocks. The screen being off counts as locked: the phone isn't in
 * anyone's hand. The choice is the app's (when_locked.txt beside the
 * identity: "paired", "ask" or "never"); this tells the core what applies
 * right now.
 */
object LockState {
    fun isLocked(context: Context): Boolean {
        val keyguard = context.getSystemService(KeyguardManager::class.java)
        val power = context.getSystemService(PowerManager::class.java)
        return keyguard?.isKeyguardLocked == true || power?.isInteractive == false
    }

    /** Apply now, and on every change for as long as the process lives. */
    fun watch(context: Context) {
        val app = context.applicationContext
        apply(app)
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_OFF)
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_USER_PRESENT)
        }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) = apply(context)
        }
        // System broadcasts only; nothing else needs to reach it.
        ContextCompat.registerReceiver(app, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
    }

    /** Tell the core what applies right now (also after the choice changes). */
    fun apply(context: Context) {
        val locked = isLocked(context)
        // Whatever the files setting: a locked phone never shares its screen.
        Core.setScreenLocked(locked)
        val mode = if (!locked) {
            "usual"
        } else {
            val choice = runCatching {
                File(Device.dataDir(context), "when_locked.txt").readText().trim()
            }.getOrNull()
            when (choice) {
                "ask" -> "ask"
                "never" -> "refuse"
                else -> "usual"
            }
        }
        Core.setReceiving(mode)
    }
}
