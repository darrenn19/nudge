package io.github.darrenn19.nudge

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import android.view.Display
import android.view.Surface
import androidx.core.app.ServiceCompat
import androidx.core.content.IntentCompat
import org.json.JSONArray
import kotlin.concurrent.thread

/**
 * Sharing this phone's screen with a paired device (screen mirroring).
 *
 * MediaProjection -> a virtual display -> the hardware H.264 encoder ->
 * [Core.mirrorSend] -> the core's real-time session (encrypted UDP, loss
 * recovery, pacing). Encoder settings are the ones measured best in
 * `spikes/mirror-latency` (docs/BENCHMARKS.md): Qualcomm's low-latency mode at
 * the highest clock (~11 ms to encode), intra-refresh instead of periodic
 * keyframes (no big bursts), 30 Mbit/s constant (lower when the network
 * can't carry it), with a quality floor for whole-screen changes ([QP_MAX]).
 * In the PC's game mode the picture is smaller ([GAME_MAX_SIDE]).
 *
 * The PC can also control the phone during the session ([RemoteControl],
 * carried out by [ControlService] once the user turns it on).
 *
 * Its notification is the "your screen is being shared" indicator, with
 * Stop. The session ends when either side stops, the phone locks (the core
 * ends it), or Android ends the capture.
 */
class MirrorService : Service() {
    companion object {
        private const val TAG = "Nudge"
        private const val EXTRA_RESULT_CODE = "resultCode"
        private const val EXTRA_DATA = "data"
        private const val EXTRA_ASK = "ask"
        private const val EXTRA_DEVICE = "device"
        private const val NOTIFICATION_ID = 6

        private const val MBPS = 30
        private const val MAX_SIDE = 2304
        private const val FPS = 60

        /**
         * Game mode on the PC: a smaller picture (about half the pixels), so
         * each frame takes about half as long to encode and to send, and the
         * same bitrate buys a cleaner picture per pixel.
         */
        private const val GAME_MAX_SIDE = 1600

        /**
         * The coarsest quantizer any frame may use (H.264: 0 finest, 51
         * coarsest). At a constant bitrate with intra-refresh, a frame that
         * changes the whole screen (next video, next home page) got only an
         * ordinary frame's bytes: it came out as coarse blocks that sharpened
         * over the next half second, as if loading in. With this limit such a
         * frame is sent sharp, as a bigger frame instead (a few ms more to
         * send, once). Ordinary frames stay well under it.
         */
        private const val QP_MAX = 38

        /** Intra-refresh: frames for one full sweep of the picture (0.5 s). */
        private const val REFRESH_FRAMES = 30

        /** Qualcomm's own low-latency switch (Snapdragon encoders). */
        private const val QTI_LOW_LATENCY = "vendor.qti-ext-enc-low-latency.enable"

        @Volatile
        private var running: MirrorService? = null

        /**
         * Start capturing, with what the screen-capture prompt returned. Then
         * either answer the question [ask] with yes, or show the screen on
         * [device].
         */
        fun start(context: Context, resultCode: Int, data: Intent, ask: Long, device: String?) {
            val intent = Intent(context, MirrorService::class.java)
                .putExtra(EXTRA_RESULT_CODE, resultCode)
                .putExtra(EXTRA_DATA, data)
                .putExtra(EXTRA_ASK, ask)
                .putExtra(EXTRA_DEVICE, device)
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, MirrorService::class.java))
        }

        /** A session where this phone shares its screen started (from the core's events). */
        fun onStarted(id: Long, device: String) {
            running?.claim(id, device)
        }

        fun onEnded(id: Long) {
            running?.let { if (it.session == id) it.finish() }
        }
    }

    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null
    private var codec: MediaCodec? = null
    private val wifiLocks = mutableListOf<WifiManager.WifiLock>()
    private val main = Handler(Looper.getMainLooper())

    @Volatile
    private var stopping = false

    /** The core's session ID, once there is one. */
    @Volatile
    private var session = -1L

    /** The PC's input for this session. */
    private var control: RemoteControl? = null

    /** The PC is in game mode: a smaller picture ([GAME_MAX_SIDE]). */
    @Volatile
    private var game = false

    /** Asked for a session (by answering yes): claim the next one that starts. */
    @Volatile
    private var awaiting = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null || running != null) return START_NOT_STICKY
        // Android 14+: the foreground service must exist before the projection.
        val type = if (Build.VERSION.SDK_INT >= 29) ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, Notifications.sharing(this, null), type)
        running = this

        val resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, 0)
        val data = IntentCompat.getParcelableExtra(intent, EXTRA_DATA, Intent::class.java)
        val ask = intent.getLongExtra(EXTRA_ASK, -1)
        val device = intent.getStringExtra(EXTRA_DEVICE)
        val mp = data?.let { getSystemService(MediaProjectionManager::class.java).getMediaProjection(resultCode, it) }
        if (mp == null) {
            if (ask >= 0) Core.answerMirror(ask, false)
            finish()
            return START_NOT_STICKY
        }
        projection = mp
        mp.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() = finish() // Android ended it (or the user, from the system)
        }, main)

        try {
            startCapture(mp)
        } catch (e: Exception) {
            Log.e(TAG, "mirroring: couldn't start capturing", e)
            if (ask >= 0) Core.answerMirror(ask, false)
            finish()
            return START_NOT_STICKY
        }
        holdWifi()
        // Nothing came of it (the question lapsed, the other side gave up).
        main.postDelayed({ if (session < 0) finish() }, 30_000)
        // The user is here (they just answered Android's capture prompt): make
        // sure the core isn't still going by a stale "locked".
        LockState.refresh(this)
        if (ask >= 0) {
            // The core starts the session and says so ("mirrorStarted"): see onStarted.
            awaiting = true
            Core.answerMirror(ask, true)
        } else if (device != null) {
            thread(name = "nudge-mirror-start") {
                val id = Core.mirrorStart(device)
                if (id < 0) {
                    main.post { finish() }
                } else {
                    started(id, device)
                }
            }
        }
        return START_NOT_STICKY
    }

    /** The core started the session this service answered yes for. */
    private fun claim(id: Long, device: String) {
        if (!awaiting || session >= 0) return
        awaiting = false
        if (Core.mirrorTake(id)) started(id, device) else finish()
    }

    private fun started(id: Long, device: String) {
        session = id
        Log.i(TAG, "mirroring: session $id with $device")
        // A fresh keyframe, for the decoder on the other side.
        requestKeyframe()
        val name = deviceName(device)
        main.post {
            if (stopping) return@post
            Notifications.notify(this, NOTIFICATION_ID, Notifications.sharing(this, name))
            control = RemoteControl(this, id, name, onControl = { controlled ->
                main.post {
                    if (!stopping) Notifications.notify(this, NOTIFICATION_ID, Notifications.sharing(this, name, controlled))
                }
            }, onGame = ::setGame).also { it.start() }
        }
    }

    private fun deviceName(device: String): String? = runCatching {
        val list = JSONArray(Core.devices())
        (0 until list.length()).map { list.getJSONObject(it) }.firstOrNull { it.getString("id") == device }?.getString("name")
    }.getOrNull()

    /** The screen's size as it's turned now, scaled to the encoding size. */
    private fun captureSize(): Pair<Int, Int> {
        val d = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        // The panel's own size (portrait on a phone), turned as the screen is.
        var sw = d.mode.physicalWidth
        var sh = d.mode.physicalHeight
        if (d.rotation == Surface.ROTATION_90 || d.rotation == Surface.ROTATION_270) {
            sw = sh.also { sh = sw }
        }
        val scale = minOf(1.0, (if (game) GAME_MAX_SIDE else MAX_SIDE).toDouble() / maxOf(sw, sh))
        return (sw * scale).toInt() / 8 * 8 to (sh * scale).toInt() / 8 * 8
    }

    /**
     * The encoder's bitrate now, kbit/s: follows the network ([Core.mirrorRate]),
     * from [MBPS] down when Wi-Fi can't carry it. A new encoder (the screen
     * turned) starts at it.
     */
    @Volatile
    private var kbps = MBPS * 1000
    private var lastRateCheck = 0L

    /** Every ~200 ms: the bitrate the network can carry now. */
    private fun followNetwork(c: MediaCodec, id: Long) {
        val now = System.nanoTime()
        if (id < 0 || now - lastRateCheck < 200_000_000L) return
        lastRateCheck = now
        val rate = Core.mirrorRate(id, MBPS * 1000)
        if (rate <= 0 || rate == kbps) return
        kbps = rate
        runCatching {
            c.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, rate * 1000) })
        }
    }

    /** The encoder in use and whether intra-refresh is on; replaced when the screen turns. */
    @Volatile
    private var encoder: Pair<MediaCodec, Boolean>? = null
    private var size = 0 to 0

    /**
     * Encoder settings, fastest first. Each step drops what some encoders
     * refuse: Samsung's Exynos encoders (Galaxy A54) accept the highest
     * clock and real-time priority when configured, then fail to start
     * ("can't get real-time resource"). So a step counts only once the
     * encoder has started.
     */
    private data class Tier(val profile: Boolean, val speedups: Boolean, val fastest: Boolean, val realtime: Boolean)

    private val tiers = listOf(
        Tier(profile = true, speedups = true, fastest = true, realtime = true),
        Tier(profile = true, speedups = true, fastest = false, realtime = true),
        Tier(profile = true, speedups = true, fastest = false, realtime = false),
        Tier(profile = true, speedups = false, fastest = false, realtime = false),
        Tier(profile = false, speedups = false, fastest = false, realtime = false),
    )

    /** The first step that worked on this phone: the screen turning starts there. */
    private var tier = 0

    /** An encoder for [w] x [h], started, with its input surface. */
    private fun startEncoder(w: Int, h: Int): Triple<MediaCodec, Boolean, android.view.Surface> {
        var failure: Exception? = null
        for (i in tier until tiers.size) {
            val t = tiers[i]
            var codec: MediaCodec? = null
            try {
                val (c, refresh) = createEncoder(w, h, t)
                codec = c
                val surface = c.createInputSurface()
                c.start()
                tier = i
                return started(c, refresh, surface)
            } catch (e: Exception) {
                Log.w(TAG, "mirroring: encoder refused $t", e)
                failure = e
                codec?.release()
            }
        }
        throw IllegalStateException("no encoder", failure)
    }

    private fun started(c: MediaCodec, refresh: Boolean, surface: android.view.Surface): Triple<MediaCodec, Boolean, android.view.Surface> {
        if (Build.VERSION.SDK_INT >= 31 && QTI_LOW_LATENCY in c.supportedVendorParameters) {
            // Some codecs only take vendor keys after start.
            runCatching { c.setParameters(Bundle().apply { putInt(QTI_LOW_LATENCY, 1) }) }
        }
        return Triple(c, refresh, surface)
    }

    private fun startCapture(mp: MediaProjection) {
        size = captureSize()
        val (w, h) = size
        val (c, refresh, surface) = startEncoder(w, h)
        codec = c
        encoder = c to refresh
        display = mp.createVirtualDisplay(
            "nudge-mirror", w, h, resources.displayMetrics.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, null,
        )
        getSystemService(DisplayManager::class.java).registerDisplayListener(turned, main)
        thread(name = "nudge-mirror") {
            try {
                // One encoder after another: the screen turning swaps it (see `turned`).
                while (!stopping) {
                    val (current, intraRefresh) = encoder ?: break
                    try {
                        encodeLoop(current, intraRefresh)
                        break // the session ended
                    } catch (e: IllegalStateException) {
                        if (encoder?.first === current) throw e // not a swap: a real failure
                    }
                }
            } catch (e: Exception) {
                if (!stopping) Log.w(TAG, "mirroring stopped", e)
            }
            main.post { finish() }
        }
    }

    /**
     * The screen turned: capture at the new shape, so the other side gets
     * a landscape picture rather than a small one inside a portrait frame.
     * A new encoder at the new size; the old one is stopped, which ends its
     * loop on the encoding thread, which moves on to the new one.
     */
    private val turned = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) {}
        override fun onDisplayRemoved(displayId: Int) {}
        override fun onDisplayChanged(displayId: Int) {
            if (displayId == Display.DEFAULT_DISPLAY) refit("turned")
        }
    }

    /** The PC turned game mode on or off: a smaller or the full picture. */
    private fun setGame(on: Boolean) {
        main.post {
            if (stopping || game == on) return@post
            game = on
            refit(if (on) "game mode" else "game mode off")
        }
    }

    /**
     * Captures at the size wanted now (the screen turned, game mode), if
     * it changed. On the main thread.
     */
    private fun refit(why: String) {
        if (stopping) return
        val now = runCatching { captureSize() }.getOrNull() ?: return
        if (now == size) return
        size = now
        val old = codec
        try {
            val (c, refresh, surface) = startEncoder(now.first, now.second)
            codec = c
            encoder = c to refresh
            display?.resize(now.first, now.second, resources.displayMetrics.densityDpi)
            display?.surface = surface
            Log.i(TAG, "mirroring: $why, now ${now.first}x${now.second}")
        } catch (e: Exception) {
            Log.w(TAG, "mirroring: couldn't change the picture ($why)", e)
            return
        }
        runCatching { old?.stop() }
        old?.release()
    }

    /** The encoder, and whether intra-refresh is really on. */
    private fun createEncoder(w: Int, h: Int, t: Tier): Pair<MediaCodec, Boolean> {
        val withProfile = t.profile
        val speedups = t.speedups
        val mime = MediaFormat.MIMETYPE_VIDEO_AVC
        val c = MediaCodec.createEncoderByType(mime)
        val codecCaps = c.codecInfo.getCapabilitiesForType(mime)
        val cbr = codecCaps.encoderCapabilities.isBitrateModeSupported(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
        val refresh = codecCaps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_IntraRefresh)
        val vendor = if (Build.VERSION.SDK_INT >= 31) c.supportedVendorParameters else null
        val qti = speedups && vendor != null && QTI_LOW_LATENCY in vendor
        val format = MediaFormat.createVideoFormat(mime, w, h).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, kbps * 1000)
            // Constant bitrate: in variable mode the encoder starved busy scenes.
            if (cbr) setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            setInteger(MediaFormat.KEY_FRAME_RATE, FPS)
            if (refresh) {
                // No periodic keyframes: every frame refreshes a slice of the picture.
                setInteger(MediaFormat.KEY_INTRA_REFRESH_PERIOD, REFRESH_FRAMES)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 3600)
            } else {
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10) // plus on demand after a loss
            }
            if (t.realtime) setInteger(MediaFormat.KEY_PRIORITY, 0) // real time
            // Keep sending ~10 fps when the screen is still: the other side sees
            // a live stream, and the encoder sharpens the still picture.
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000)
            if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            if (Build.VERSION.SDK_INT >= 29) setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            if (Build.VERSION.SDK_INT >= 31) {
                // Sharp whole-screen changes (see QP_MAX); encoders without it ignore it.
                setInteger(MediaFormat.KEY_VIDEO_QP_I_MAX, QP_MAX)
                setInteger(MediaFormat.KEY_VIDEO_QP_P_MAX, QP_MAX)
            }
            if (speedups) {
                // The encoder at its highest clock, each frame out as soon as it's done.
                // Highest clock where allowed; else just enough for the frame rate.
                setInteger(MediaFormat.KEY_OPERATING_RATE, if (t.fastest) Short.MAX_VALUE.toInt() else FPS)
                if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LATENCY, 1)
                if (qti) setInteger(QTI_LOW_LATENCY, 1)
            }
            // Say exactly which colours are meant, so the PC converts them back correctly.
            setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709)
            setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED)
            setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO)
            if (withProfile) {
                // High: noticeably sharper than Baseline at the same bitrate.
                setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileHigh)
                setInteger(MediaFormat.KEY_LEVEL, MediaCodecInfo.CodecProfileLevel.AVCLevel51)
            }
        }
        try {
            c.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        } catch (e: Exception) {
            c.release()
            throw e
        }
        Log.i(TAG, "mirroring: ${c.name} ${w}x$h ${kbps / 1000} Mbit/s, intra-refresh $refresh, game mode $game, $t (Qualcomm $qti)")
        return c to refresh
    }

    private fun requestKeyframe() {
        runCatching { codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) }) }
    }

    private fun encodeLoop(c: MediaCodec, refresh: Boolean) {
        val info = MediaCodec.BufferInfo()
        var csd: ByteArray? = null
        var lastPoll = 0L
        while (!stopping) {
            val index = c.dequeueOutputBuffer(info, 10_000)
            val id = session
            followNetwork(c, id)
            if (index < 0) {
                // Nothing new (a still screen): check the session now and then.
                val now = System.nanoTime()
                if (id >= 0 && now - lastPoll > 200_000_000L) {
                    lastPoll = now
                    when (Core.mirrorPoll(id)) {
                        1 -> requestKeyframe()
                        -1 -> return
                    }
                }
                continue
            }
            val out = c.getOutputBuffer(index) ?: continue
            val bytes = ByteArray(info.size)
            out.position(info.offset)
            out.get(bytes)
            c.releaseOutputBuffer(index, false)
            if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                csd = bytes // SPS/PPS: sent in front of every keyframe
                continue
            }
            if (id < 0) continue // no session yet
            val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
            val payload = if (key && csd != null) csd + bytes else bytes
            // For surface input, the timestamp is when the compositor made the frame.
            val ageUs = System.nanoTime() / 1000 - info.presentationTimeUs
            when (Core.mirrorSend(id, payload, key, refresh, ageUs)) {
                1 -> requestKeyframe()
                -1 -> return // the session ended
            }
        }
    }

    /**
     * Keeps Wi-Fi out of power saving while mirroring: otherwise it naps
     * between packets and wakes on the router's schedule (~100 ms).
     */
    private fun holdWifi() {
        val wm = applicationContext.getSystemService(WifiManager::class.java) ?: return
        val modes = buildList {
            if (Build.VERSION.SDK_INT >= 29) add(WifiManager.WIFI_MODE_FULL_LOW_LATENCY)
            @Suppress("DEPRECATION")
            add(WifiManager.WIFI_MODE_FULL_HIGH_PERF)
        }
        for (mode in modes) {
            runCatching {
                wm.createWifiLock(mode, "nudge:mirror").apply {
                    setReferenceCounted(false)
                    acquire()
                    wifiLocks += this
                }
            }
        }
    }

    private fun finish() {
        if (stopping) return
        stopping = true
        stopSelf()
    }

    override fun onDestroy() {
        stopping = true
        running = null
        control?.stop()
        if (session >= 0) Core.mirrorClose(session)
        wifiLocks.forEach { if (it.isHeld) it.release() }
        wifiLocks.clear()
        runCatching { getSystemService(DisplayManager::class.java).unregisterDisplayListener(turned) }
        display?.release()
        runCatching { codec?.stop() }
        codec?.release()
        projection?.stop()
        super.onDestroy()
    }
}
