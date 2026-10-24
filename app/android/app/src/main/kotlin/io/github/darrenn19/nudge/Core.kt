package io.github.darrenn19.nudge

import androidx.annotation.Keep

/**
 * The Rust core, run by the app's process itself (not by Flutter): started
 * with the process by [NudgeApplication], kept alive by [NudgeService].
 * The window attaches to the same core when it opens.
 *
 * Implemented in app/rust/src/android.rs.
 */
object Core {
    init {
        System.loadLibrary("nudge_bridge")
    }

    /**
     * Hears every event the core has, as JSON with a "type" (see [Background]).
     * Only Rust calls it, by name, so release builds must keep it as it is.
     */
    @Keep
    fun interface Listener {
        @Keep
        fun onEvent(json: String)
    }

    /** Start the core (or attach to it). Returns null, or what went wrong. Blocks briefly. */
    @JvmStatic external fun start(dataDir: String, downloadDir: String, deviceName: String, listener: Listener): String?

    /** Every known device, as a JSON array of {id, name, paired, online, reachable, clipboard}. */
    @JvmStatic external fun devices(): String

    @JvmStatic external fun answerOffer(id: Long, accept: Boolean)

    @JvmStatic external fun answerPair(id: Long, accept: Boolean)

    /** "usual", "ask" or "refuse". */
    @JvmStatic external fun setReceiving(mode: String)

    /** Answer a "hotspotWanted" event; null [ssid] if the hotspot didn't start. */
    @JvmStatic external fun hotspotReady(id: Long, ssid: String?, passphrase: String, wpa3: Boolean)

    @JvmStatic external fun warm(deviceId: String)

    /** Send open descriptors (ownership passes to the core). */
    @JvmStatic external fun sendOpenFiles(deviceId: String, fds: IntArray, names: Array<String>): Boolean

    /**
     * Put [text] on [deviceId]'s clipboard, or every syncing device's with
     * null. Returns the names reached as a JSON array. Blocks: not on the
     * main thread.
     */
    @JvmStatic external fun sendClipboard(deviceId: String?, text: String): String

    /** The same for a copied image (`mime` like "image/png"). Blocks. */
    @JvmStatic external fun sendClipboardImage(deviceId: String?, mime: String, data: ByteArray): String
}
