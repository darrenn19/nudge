package io.github.darrenn19.nudge

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** The core's devices, and which one to send to without asking. */
object Targets {
    fun devices(): List<JSONObject> {
        val list = runCatching { JSONArray(Core.devices()) }.getOrNull() ?: return emptyList()
        return (0 until list.length()).map { list.getJSONObject(it) }
    }

    /** A device's name, or a stand-in. */
    fun name(id: String): String =
        devices().find { it.getString("id") == id }?.getString("name") ?: "your device"

    /**
     * The device to send to without asking, as (id, name): the one nearby
     * (paired, or a browser page), or, with none nearby, the one paired
     * device seen earlier. Null when there's a choice. Same rule as the app.
     */
    fun onlyChoice(): Pair<String, String>? = onlyChoice(devices())

    private fun onlyChoice(all: List<JSONObject>): Pair<String, String>? {
        val nearby = all.filter {
            it.getBoolean("online") && (it.getBoolean("paired") || it.optString("platform") == "browser")
        }
        val reachable = all.filter { it.getBoolean("paired") && it.getBoolean("reachable") }
        val pick = when {
            nearby.size == 1 -> nearby.single()
            nearby.isEmpty() && reachable.size == 1 -> reachable.single()
            else -> return null
        }
        return pick.getString("id") to pick.getString("name")
    }

    /**
     * Where "Send latest" goes: the default device if the user set one and
     * it can be reached; else the one obvious device; else the current
     * device (the one last swiped to in the app). Null: let the user choose.
     * The app keeps both choices in send_target.json (see the controller).
     */
    fun forLatest(context: Context): Pair<String, String>? {
        val all = devices()
        val saved = runCatching {
            JSONObject(File(Device.dataDir(context), "send_target.json").readText())
        }.getOrNull()
        fun usable(key: String): Pair<String, String>? {
            val id = saved?.optString(key)?.takeIf { it.isNotEmpty() && it != "null" } ?: return null
            val d = all.find { it.getString("id") == id } ?: return null
            val ok = d.getBoolean("paired") && (d.getBoolean("online") || d.getBoolean("reachable"))
            return if (ok) id to d.getString("name") else null
        }
        return usable("default") ?: onlyChoice(all) ?: usable("current")
    }
}
