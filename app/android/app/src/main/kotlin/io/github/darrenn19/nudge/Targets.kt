package io.github.darrenn19.nudge

import org.json.JSONArray
import org.json.JSONObject

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
    fun onlyChoice(): Pair<String, String>? {
        val all = devices()
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
}
