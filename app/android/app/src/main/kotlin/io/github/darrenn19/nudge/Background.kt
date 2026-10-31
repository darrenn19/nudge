package io.github.darrenn19.nudge

import android.util.Log

import android.content.Context
import android.media.MediaScannerConnection
import android.os.Handler
import android.os.Looper
import androidx.annotation.Keep
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.Locale

/**
 * What Nudge does with no window: everything the core reports, turned into
 * notifications (progress, "received", "wants to send you"), clipboard text
 * put in place, and the hotspot started and stopped. Runs on the main
 * thread, fed by [Core]'s listener.
 *
 * With the window in front ([inFront]), questions and arrivals are shown
 * in the app instead, which hears the same events.
 */
object Background : Core.Listener {
    /** The window is on screen (set by [MainActivity]). */
    @Volatile var inFront = false

    private lateinit var context: Context
    private val main = Handler(Looper.getMainLooper())

    private class Transfer(
        val device: String,
        val incoming: Boolean,
        val name: String,
        val size: Long,
    ) {
        var bytes = 0L
        var speed = 0.0
        var finished = false
        var delivered = false
        var pending = false
        var accepted = false

        /** Bytes are moving (not waiting on anyone's answer). */
        val active: Boolean
            get() = !finished && !(incoming && delivered) && !(!incoming && pending && !accepted && delivered)
    }

    private val transfers = LinkedHashMap<Long, Transfer>()
    private var names = mapOf<String, String>()
    private var showing = false
    private var holding = false
    private var updateQueued = false
    private val idle = Runnable { goIdle() }

    fun start(context: Context) {
        this.context = context.applicationContext
    }

    @Keep
    override fun onEvent(json: String) {
        main.post { handle(JSONObject(json)) }
    }

    private fun handle(e: JSONObject) {
        val id = e.optLong("id")
        when (e.getString("type")) {
            "devices" -> names = emptyMap()
            "started" -> transfers[id] = Transfer(
                e.getString("device"),
                e.getBoolean("incoming"),
                e.getString("name"),
                e.getLong("size"),
            )
            "progress" -> transfers[id]?.let {
                it.bytes = e.getLong("bytes")
                val speed = e.getDouble("speed")
                it.speed = if (it.speed == 0.0) speed else it.speed * 0.6 + speed * 0.4
            }
            "done" -> transfers.remove(id)?.let { t ->
                if (t.incoming) received(e.getString("path"), name(t.device), t.name)
            }
            "failed" -> transfers.remove(id)
            "delivered" -> transfers[id]?.let {
                it.delivered = true
                it.bytes = it.size
            }
            "pending" -> transfers[id]?.pending = true
            "accepted" -> transfers[id]?.accepted = true
            "offer" -> if (!inFront) {
                val count = e.getInt("count")
                val name = e.getString("name")
                val size = e.getLong("size")
                Notifications.offer(
                    context,
                    id,
                    "${name(e.getString("device"))} wants to send you ${if (count == 1) name else "$count files"}",
                    if (count == 1) formatSize(size) else "$name and more · ${formatSize(size)}",
                )
            }
            "pairAsked" -> if (!inFront) {
                Notifications.offer(
                    context,
                    id,
                    "${e.getString("name")} wants to pair",
                    "Check it shows ${e.getString("code")}",
                    pairing = true,
                )
            }
            // A paired device wants to see the screen: it just starts (the
            // Pair screen warns to pair only your own devices; "Your screen
            // is being shared" shows meanwhile). Android's own capture prompt
            // still comes unless Nudge was given PROJECT_MEDIA over adb. A
            // locked phone is asked instead: answering needs an unlock, and
            // nothing is shared while locked: it starts once it's unlocked
            // (the notification is there in case it can't).
            "mirrorAsked" -> if (!LockState.isLocked(context) && MirrorConsentActivity.answerYes(context, id)) {
                Log.i("Nudge", "mirroring: ${name(e.getString("device"))} asked to see the screen; sharing")
            } else {
                Notifications.offer(
                    context,
                    id,
                    "${name(e.getString("device"))} wants to see your screen",
                    "Unlock to share it",
                    mirror = true,
                )
                if (LockState.isLocked(context)) MirrorConsentActivity.answerOnUnlock(context, id)
            }
            // MirrorService captures the screen and shows "being shared".
            "mirrorStarted" -> if (e.getBoolean("sharing")) MirrorService.onStarted(id, e.getString("device"))
            "mirrorEnded" -> MirrorService.onEnded(id)
            "offerClosed" -> Notifications.cancelOffer(context, id)
            "clipboard" -> ClipSync.set(context, e.getString("text"))
            "clipboardImage" -> ClipSync.setImage(context, java.io.File(e.getString("path")), e.getString("mime"))
            "hotspotWanted" -> Hotspot.start(context, directOnly = e.optBoolean("directOnly")) { h ->
                Core.hotspotReady(
                    id,
                    h?.get("ssid") as String?,
                    h?.get("passphrase") as String? ?: "",
                    h?.get("wpa3") == true,
                    h?.get("mhz") as Int? ?: 0,
                )
            }
            "hotspotDone" -> Hotspot.stop()
        }
        if (!updateQueued) {
            // Progress comes many times a second; the notification needs one.
            updateQueued = true
            main.postDelayed({
                updateQueued = false
                update()
            }, 800)
        }
    }

    /** A device's name, or a stand-in. */
    private fun name(device: String): String {
        if (device !in names) {
            names = runCatching {
                val list = JSONArray(Core.devices())
                (0 until list.length()).associate {
                    val d = list.getJSONObject(it)
                    d.getString("id") to d.getString("name")
                }
            }.getOrDefault(emptyMap())
        }
        return names[device] ?: "A paired device"
    }

    /** Into Files and Gallery; a notification unless the app says it itself. */
    /**
     * [title] is what the sender called it: a file's name, or for several
     * files sent as one ("Trip", "37 files") the folder they went in is
     * [path], which the media scanner walks.
     */
    private fun received(path: String, from: String, title: String) {
        MediaScannerConnection.scanFile(context, arrayOf(path), null, null)
        if (!inFront) Notifications.received(context, File(path), from, title)
    }

    private fun update() {
        val running = transfers.values.filter { it.active }
        // Wi-Fi at full speed for as long as anything moves, on screen or
        // not (asked again each time, which keeps the CPU lock from timing out).
        if (running.isNotEmpty() || holding) NudgeService.holdNetwork(context, running.isNotEmpty())
        holding = running.isNotEmpty()
        // On screen the app shows progress itself.
        val shown = if (inFront) emptyList() else running
        if (shown.isEmpty()) {
            // Multi-file sends have a short gap between files; don't flicker.
            main.removeCallbacks(idle)
            main.postDelayed(idle, 1500)
            return
        }
        main.removeCallbacks(idle)
        showing = true
        val size = shown.sumOf { it.size }
        val bytes = shown.sumOf { it.bytes }
        val speed = shown.sumOf { it.speed }
        val title = if (shown.size == 1) {
            val t = shown.single()
            "${if (t.incoming) "Receiving" else "Sending"} ${t.name} ${if (t.incoming) "from" else "to"} ${name(t.device)}"
        } else {
            val verb = when {
                shown.all { it.incoming } -> "Receiving"
                shown.none { it.incoming } -> "Sending"
                else -> "Moving"
            }
            "$verb ${shown.size} files"
        }
        val text = buildList {
            add("${formatSize(bytes)} of ${formatSize(size)}")
            if (speed > 0) {
                add("${formatSize(speed.toLong())}/s")
                add("${formatDuration(((size - bytes) / speed).toLong())} left")
            }
        }.joinToString(" · ")
        val percent = if (size == 0L || bytes == 0L) -1 else (bytes * 100 / size).toInt()
        val ids = transfers.filterValues { it.active }.keys.toLongArray()
        NudgeService.showProgress(context, title, text, percent, ids)
    }

    private fun goIdle() {
        if (showing && transfers.values.none { it.active }) {
            showing = false
            NudgeService.idle(context)
        }
    }

    /** Like the app's formatSize: "4.2 MB". */
    fun formatSize(bytes: Long): String {
        val units = listOf("B", "KB", "MB", "GB", "TB")
        var value = bytes.toDouble()
        var unit = 0
        while (value >= 1000 && unit < units.size - 1) {
            value /= 1000
            unit++
        }
        if (unit == 0) return "$bytes B"
        return String.format(Locale.US, if (value >= 100) "%.0f %s" else "%.1f %s", value, units[unit])
    }

    private fun formatDuration(seconds: Long): String = when {
        seconds < 60 -> "${seconds.coerceIn(1, 59)} s"
        seconds < 3600 -> "${seconds / 60} min"
        seconds % 3600 / 60 == 0L -> "${seconds / 3600} h"
        else -> "${seconds / 3600} h ${seconds % 3600 / 60} min"
    }
}
