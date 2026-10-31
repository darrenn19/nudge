package io.github.darrenn19.nudge

import android.app.KeyguardManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import androidx.core.content.ContextCompat
import java.io.File

/**
 * The "Accept files while locked" setting, applied as the phone locks and
 * unlocks. The screen being off counts as locked: the phone isn't in
 * anyone's hand. The choice is the app's (when_locked.txt beside the
 * identity: "paired", "ask" or "never"); this tells the core what applies
 * right now.
 *
 * Android's own reading can't be trusted alone: at "user present" the
 * keyguard can still report locked (Samsung, fingerprint unlock) and no
 * later broadcast corrects it, which left the phone refusing to share its
 * screen while in the user's hand. So the broadcasts lead:
 * - screen off: locked (the phone always locks with the screen off);
 * - user present: unlocked, until the screen goes off again;
 * - otherwise (screen on at the lock screen, or at start): the keyguard.
 * Anything that depends on it reads it fresh first ([refresh]).
 */
object LockState {
    private val main = Handler(Looper.getMainLooper())

    /** The user unlocked since the screen last went off. */
    @Volatile
    private var present: Boolean? = null

    /** What the core was last told, for the log. */
    @Volatile
    private var told: Boolean? = null

    fun isLocked(context: Context): Boolean {
        val keyguard = context.getSystemService(KeyguardManager::class.java)
        val power = context.getSystemService(PowerManager::class.java)
        if (power?.isInteractive == false) return true
        if (present == true) return false
        return keyguard?.isKeyguardLocked == true
    }

    /** Apply now, and on every change for as long as the process lives. */
    fun watch(context: Context) {
        val app = context.applicationContext
        val keyguard = app.getSystemService(KeyguardManager::class.java)
        if (present == null) present = keyguard?.isKeyguardLocked == false
        apply(app)
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_OFF)
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_USER_PRESENT)
        }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                when (intent.action) {
                    Intent.ACTION_SCREEN_OFF -> present = false
                    Intent.ACTION_USER_PRESENT -> present = true
                }
                apply(context)
                // A phone with no lock screen (or Smart Lock) may unlock
                // without a "user present": read the keyguard again shortly.
                if (intent.action == Intent.ACTION_SCREEN_ON) {
                    for (delay in longArrayOf(1_000, 3_000)) main.postDelayed({ apply(context) }, delay)
                }
            }
        }
        // System broadcasts only; nothing else needs to reach it.
        ContextCompat.registerReceiver(app, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
    }

    /** Read the lock state again and tell the core, right now. */
    fun refresh(context: Context) = apply(context.applicationContext)

    /** Tell the core what applies right now (also after the choice changes). */
    fun apply(context: Context) {
        val locked = isLocked(context)
        if (told != locked) Log.i("Nudge", "lock state: ${if (locked) "locked" else "unlocked"}")
        told = locked
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
