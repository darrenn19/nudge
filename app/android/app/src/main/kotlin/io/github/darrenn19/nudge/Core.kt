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

    /** Stop a transfer and the rest of its send. */
    @JvmStatic external fun cancel(id: Long)

    @JvmStatic external fun answerPair(id: Long, accept: Boolean)

    /** Answer a "mirrorAsked" event: may that device see this screen? */
    @JvmStatic external fun answerMirror(id: Long, accept: Boolean)

    /** End a screen-mirroring session. */
    @JvmStatic external fun stopMirror(id: Long)

    /** Claim the session behind a "mirrorStarted" event. False if it's gone. */
    @JvmStatic external fun mirrorTake(id: Long): Boolean

    /** Show this phone's screen on [deviceId]. Blocks until it answers: the session ID, or -1. */
    @JvmStatic external fun mirrorStart(deviceId: String): Long

    /**
     * Send one encoded frame, captured [ageUs] ago. Returns 0, 1 if the other
     * side wants a keyframe, or -1 once the session has ended.
     */
    @JvmStatic external fun mirrorSend(id: Long, data: ByteArray, key: Boolean, intraRefresh: Boolean, ageUs: Long): Int

    /** 0 running, 1 keyframe wanted, -1 ended. For a still screen (no frames to send). */
    @JvmStatic external fun mirrorPoll(id: Long): Int

    /** Tell the PC this phone is unlocked and its user is being asked to allow sharing again. */
    @JvmStatic external fun mirrorResuming(id: Long)

    /** 10 ms of this phone's sound for the PC (raw PCM, the first [length] bytes). 0, or -1 once ended. */
    @JvmStatic external fun mirrorSendAudio(id: Long, data: ByteArray, length: Int): Int

    /** End a session. */
    @JvmStatic external fun mirrorClose(id: Long)

    /**
     * The PC's next input message for session [id] (core/src/input.rs format),
     * waiting up to [timeoutMs]. Null if none came; empty once the session ended.
     */
    @JvmStatic external fun mirrorInput(id: Long, timeoutMs: Int): ByteArray?

    /**
     * The encoder's bitrate for the network now, kbit/s (new value), or 0 to
     * keep it. Call every ~200 ms; [maxKbps] is the top rate.
     */
    @JvmStatic external fun mirrorRate(id: Long, maxKbps: Int): Int

    /** Tell the PC whether it can control this phone: 1 ready, 2 needs the Accessibility switch, 0 can't. */
    @JvmStatic external fun mirrorControlStatus(id: Long, status: Int)

    /** Locked or screen off: the screen is never shared meanwhile. */
    @JvmStatic external fun setScreenLocked(locked: Boolean)

    /** "usual", "ask" or "refuse". */
    @JvmStatic external fun setReceiving(mode: String)

    /** Plugged into a computer with USB tethering off (see [UsbCable]). */
    @JvmStatic external fun setCableUnused(on: Boolean)

    /**
     * Answer a "hotspotWanted" event; null [ssid] if no direct link started.
     * [mhz]: its frequency, 0 if unknown.
     */
    @JvmStatic external fun hotspotReady(id: Long, ssid: String?, passphrase: String, wpa3: Boolean, mhz: Int)

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
