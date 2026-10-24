package io.github.darrenn19.nudge

import android.Manifest
import android.content.ClipboardManager
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.PixelFormat
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.WindowManager

/**
 * Sends what's copied on the phone to your other devices the moment it's
 * copied, with Nudge in the background.
 *
 * Android (10+) lets only the app in front read the clipboard, and doesn't
 * even tell background apps it changed. Two workarounds, as KDE Connect does:
 *  - Noticing: Android logs "Denying clipboard access to <app>" each time it
 *    skips telling us about a change. Reading the log needs READ_LOGS, which
 *    can only be granted from a computer (adb), once.
 *  - Reading: a 1-pixel invisible window takes focus for a moment (needs
 *    "Appear on top"); while it has focus, the clipboard can be read.
 *
 * Without both permissions this does nothing, and the notification's "Send
 * clipboard" button and opening Nudge still work.
 */
object ClipWatch {
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var reader: Thread? = null

    /** Changes Nudge made itself (text from another device) aren't sent back. */
    @Volatile private var quietUntil = 0L

    private var grabbing: View? = null

    /** Registering is what makes Android log the denial we watch for. */
    private val listener = ClipboardManager.OnPrimaryClipChangedListener { }

    fun canDrawOverlay(context: Context) = Settings.canDrawOverlays(context)

    fun canReadLogs(context: Context) =
        context.checkSelfPermission(Manifest.permission.READ_LOGS) == PackageManager.PERMISSION_GRANTED

    /** Watching, and Android lets it see copies. */
    fun running() = reader?.isAlive == true && allowed

    @Volatile private var process: Process? = null

    /**
     * Whether Android lets the reader see other apps' lines. Since Android
     * 13 it asks "Allow access to all device logs?" when the reader starts,
     * and only while Nudge is on screen; otherwise (or if declined) the
     * reader sees only Nudge's own lines, with no error. Seeing any line
     * from another process proves it was allowed.
     */
    @Volatile private var allowed = false

    /** The reader started while Nudge was on screen, so Android asked. */
    @Volatile private var askedOnScreen = false

    private val pidOf = Regex("""\(\s*(\d+)\)""")

    /**
     * Start watching, if the permissions are there. Called whenever Nudge
     * opens: a reader Android never asked about (started while Nudge wasn't
     * on screen) is replaced, so it asks. With [again], also one it asked
     * about and was refused (the user chose to try again).
     */
    fun start(context: Context, again: Boolean = false) {
        val app = context.applicationContext
        if (!canDrawOverlay(app) || !canReadLogs(app)) return
        val alive = reader?.isAlive == true
        if (alive && (allowed || askedOnScreen && !again)) return
        askedOnScreen = Background.inFront
        process?.destroy()
        reader?.interrupt()
        allowed = false
        main.post {
            val clipboard = app.getSystemService(ClipboardManager::class.java) ?: return@post
            clipboard.removePrimaryClipChangedListener(listener)
            clipboard.addPrimaryClipChangedListener(listener)
        }
        val me = app.packageName
        val myPid = android.os.Process.myPid().toString()
        reader = Thread({
            val started = runCatching {
                // From now on: the clipboard service's refusals, and app
                // launches (frequent, so they show whether we're allowed).
                ProcessBuilder(
                    "logcat", "-T", "1", "-v", "brief",
                    "ClipboardService:E", "ActivityTaskManager:I", "*:S",
                ).redirectErrorStream(true).start()
            }.getOrNull() ?: return@Thread
            process = started
            try {
                started.inputStream.bufferedReader().useLines { lines ->
                    Log.i(TAG, "watching the clipboard")
                    for (line in lines) {
                        val pid = pidOf.find(line)?.groupValues?.get(1)
                        if (!allowed && pid != null && pid != myPid) {
                            allowed = true
                            Notifications.clipboardResumed(app)
                            Log.i(TAG, "log access allowed")
                        }
                        if (line.contains("Denying clipboard access to $me")) changed(app)
                    }
                    Log.i(TAG, "log closed")
                }
            } catch (_: Exception) {
                // The log closed; start() runs again when Nudge opens.
            } finally {
                started.destroy()
            }
        }, "nudge-clipwatch").apply {
            isDaemon = true
            start()
        }
    }

    /**
     * The background service started (after a reboot, an update...): if
     * instant clipboard is set up, say it's paused until Nudge is opened.
     */
    fun serviceStarted(context: Context) {
        if (canDrawOverlay(context) && canReadLogs(context) && !running()) {
            Notifications.clipboardPaused(context)
        }
    }

    /** Nudge is about to put text from another device on the clipboard. */
    fun quietForOwnChange() {
        quietUntil = SystemClock.uptimeMillis() + 1500
    }

    private var pending = false

    /** Something was copied (or cut, or cleared). Several log lines can come at once. */
    private fun changed(context: Context) {
        val quiet = SystemClock.uptimeMillis() < quietUntil
        Log.i(TAG, if (quiet) "clipboard changed (by Nudge)" else "clipboard changed")
        if (quiet) return
        main.post {
            if (pending) return@post
            pending = true
            main.postDelayed({
                pending = false
                grab(context)
            }, 60)
        }
    }

    /** Take focus for a moment with an invisible window, read, let go. */
    private fun grab(context: Context) {
        Log.i(TAG, "copy noticed")
        if (grabbing != null || !canDrawOverlay(context)) return
        val windows = context.getSystemService(WindowManager::class.java) ?: return
        val view = object : View(context) {
            override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
                super.onWindowFocusChanged(hasWindowFocus)
                if (hasWindowFocus) {
                    // Only the reading needs focus; sending (and reading an
                    // image's bytes) happens after.
                    val item = ClipSync.copiedItem(context)
                    Log.i(TAG, if (item == null) "nothing new to send" else "sending")
                    // Not from inside the focus callback: Android is still
                    // handling it, and removing the window now crashes.
                    main.post { release(windows) }
                    if (item != null) Thread { ClipSync.send(context, item) }.start()
                }
            }
        }
        val params = WindowManager.LayoutParams(
            1,
            1,
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            // Focusable (that's the point), but touches pass through.
            WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                WindowManager.LayoutParams.FLAG_ALT_FOCUSABLE_IM,
            PixelFormat.TRANSLUCENT,
        ).apply { gravity = Gravity.TOP or Gravity.START }
        try {
            windows.addView(view, params)
            grabbing = view
        } catch (e: Exception) {
            Log.w(TAG, "no window: $e")
            return
        }
        // Never keep focus for long, whatever happens.
        main.postDelayed({ release(windows) }, 800)
    }

    private fun release(windows: WindowManager) {
        val view = grabbing ?: return
        grabbing = null
        runCatching { windows.removeView(view) }
    }

    private const val TAG = "NudgeClip"
}
