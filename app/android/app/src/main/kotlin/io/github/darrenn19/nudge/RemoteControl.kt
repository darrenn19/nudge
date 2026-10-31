package io.github.darrenn19.nudge

import android.content.Context
import android.util.Log
import kotlin.concurrent.thread

/**
 * The PC's input during one mirroring session where this phone shares its
 * screen: read from the session ([Core.mirrorInput]) and handed to
 * [ControlService], which carries it out.
 *
 * The PC hears whether control works ([Core.mirrorControlStatus]) once it
 * has sent something (so a PC that doesn't know about control never gets a
 * message it can't read), and again when it changes. If Nudge's
 * Accessibility switch is off, the first input shows a notification that
 * leads there; nothing is carried out until it's on.
 *
 * Wire format: `nudge_core::input` (core/src/input.rs).
 */
class RemoteControl(
    private val context: Context,
    private val session: Long,
    private val device: String?,
    /** Control became possible (true) or stopped being (false). */
    private val onControl: (Boolean) -> Unit,
    /** The PC turned game mode on (true) or off. */
    private val onGame: (Boolean) -> Unit = {},
) {
    companion object {
        private const val TAG = "Nudge"
        private const val WAIT_MS = 250

        /** A held pointer with no word for this long is lifted (`input::HOLD_TIMEOUT_MS`). */
        private const val HOLD_TIMEOUT_MS = 1000L

        private const val READY = 1
        private const val NEEDS_SETUP = 2

        /** The `Mode` message and its game-mode flag. */
        private const val MODE = 8
        private const val GAME = 1
    }

    @Volatile
    private var stopped = false

    /** What the PC was last told; 0 until it has sent something. */
    private var told = 0
    private var asked = false

    fun start() {
        thread(name = "nudge-control-in") {
            android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_URGENT_DISPLAY)
            try {
                loop()
            } catch (e: Exception) {
                Log.w(TAG, "control: input stopped", e)
            }
            ControlService.instance?.release()
            Notifications.cancel(context, Notifications.CONTROL_SETUP_ID)
        }
    }

    fun stop() {
        stopped = true
    }

    private fun loop() {
        while (!stopped) {
            val m = Core.mirrorInput(session, WAIT_MS)
            if (m != null && m.isEmpty()) return // the session ended
            val service = ControlService.instance
            if (m != null && told == 0) told = -1 // heard from the PC: it can be told now
            if (told != 0) status(service)
            if (m == null) {
                service?.liftIfQuiet(HOLD_TIMEOUT_MS)
                continue
            }
            // The PC's mode is about the picture: it counts with control off too.
            if (m[0].toInt() == MODE) {
                if (m.size >= 2) onGame(m[1].toInt() and GAME != 0)
                continue
            }
            if (service == null) {
                askToTurnOn()
                continue
            }
            handle(service, m)
        }
    }

    private fun status(service: ControlService?) {
        val now = if (service != null) READY else NEEDS_SETUP
        if (now == told) return
        told = now
        Core.mirrorControlStatus(session, now)
        if (now == READY) Notifications.cancel(context, Notifications.CONTROL_SETUP_ID)
        onControl(now == READY)
    }

    /** Once per session: the switch is off, and the PC is trying to control the phone. */
    private fun askToTurnOn() {
        if (asked) return
        asked = true
        Log.i(TAG, "control: the PC wants control; Accessibility switch is off")
        Notifications.notify(context, Notifications.CONTROL_SETUP_ID, Notifications.controlSetup(context, device))
        // Straight to the switch where Android allows it (Nudge may draw over
        // other apps, which lets it open a screen from the background).
        if (android.provider.Settings.canDrawOverlays(context)) {
            runCatching {
                context.startActivity(ControlService.settingsIntent(context).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK))
            }
        }
    }

    private fun handle(s: ControlService, m: ByteArray) {
        fun u16(i: Int) = ((m[i].toInt() and 0xFF) shl 8) or (m[i + 1].toInt() and 0xFF)
        fun s16(i: Int) = u16(i).toShort().toInt()
        fun frac(v: Int) = v / 65535f
        when (m[0].toInt()) {
            1 -> if (m.size >= 7) s.touch(m[2].toInt() and 0xFF, m[1].toInt(), frac(u16(3)), frac(u16(5)))
            2 -> if (m.size >= 9) s.scroll(frac(u16(1)), frac(u16(3)), s16(5), s16(7))
            3 -> if (m.size >= 2) s.key(m[1].toInt() and 0xFF, if (m.size >= 3) m[2].toInt() and 0xFF else 0)
            4 -> s.text(String(m, 1, m.size - 1, Charsets.UTF_8))
            6 -> if (m.size >= 9) s.pan(frac(u16(1)), frac(u16(3)), s16(5) / 32767f, s16(7) / 32767f)
            7 -> s.panEnd()
            9 -> if (m.size >= 9) s.swipe(frac(u16(1)), frac(u16(3)), s16(5) / 100f, s16(7) / 100f)
        }
    }
}
