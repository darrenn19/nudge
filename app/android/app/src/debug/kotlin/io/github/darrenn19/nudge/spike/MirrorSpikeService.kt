package io.github.darrenn19.nudge.spike

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
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
import android.view.WindowManager
import androidx.core.app.ServiceCompat
import androidx.core.content.IntentCompat
import io.github.darrenn19.nudge.R
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketAddress
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.locks.LockSupport
import kotlin.concurrent.thread

/**
 * Screen-mirroring latency spike (throwaway, debug builds only).
 *
 * MediaProjection -> VirtualDisplay -> hardware H.264 encoder (low-latency mode)
 * -> plain UDP to the PC's `mirror-latency` tool. Paced sending, and missing
 * packets are resent when the PC asks (NACK). No encryption.
 */
class MirrorSpikeService : Service() {
    companion object {
        const val EXTRA_RESULT_CODE = "resultCode"
        const val EXTRA_DATA = "data"
        const val EXTRA_HOST = "host"
        const val EXTRA_MBPS = "mbps"
        const val EXTRA_MAX_SIDE = "maxSide"
        const val EXTRA_HEVC = "hevc"
        const val EXTRA_PACE_MBPS = "paceMbps"
        const val EXTRA_INTRA_REFRESH = "intraRefresh"
        const val EXTRA_SPEEDUPS = "speedups"

        @Volatile
        var status = "Idle"

        private const val TAG = "MirrorSpike"
        private const val PORT = 47900
        private const val CHUNK = 1200
        private const val HEADER = 26
        private const val T_FRAG: Byte = 1
        private const val T_PONG: Byte = 2
        private const val T_HELLO: Byte = 3
        private const val T_INFO: Byte = 4
        private const val T_PING: Byte = 10
        private const val T_KEY: Byte = 11
        private const val T_NACK: Byte = 12
        private const val F_KEY = 1
        private const val F_HEVC = 2
        private const val F_RESENT = 4
        private const val F_INTRA_REFRESH = 8

        /**
         * Frames kept for resends: 1 s at 60 fps. Wi-Fi can hold everything for
         * a few hundred ms; the PC's requests arrive after that, and must still
         * find the packets here.
         */
        private const val HISTORY = 60

        /** Intra-refresh: frames for one full sweep of the picture (0.5 s at 60 fps). */
        private const val REFRESH_FRAMES = 30

        /** Qualcomm's own low-latency switch (Snapdragon encoders). */
        private const val QTI_LOW_LATENCY = "vendor.qti-ext-enc-low-latency.enable"
    }

    @Volatile
    private var running = false
    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null
    private var codec: MediaCodec? = null
    private var socket: DatagramSocket? = null
    private val info = mutableListOf<String>()
    private val wifiLocks = mutableListOf<WifiManager.WifiLock>()

    /** Recent frames' packets by frame id, for resends. */
    private val history = object : LinkedHashMap<Int, Array<ByteArray>>() {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Int, Array<ByteArray>>?) = size > HISTORY
    }

    @Volatile
    private var nacks = 0

    @Volatile
    private var resent = 0

    @Volatile
    private var keyRequests = 0

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null || running) return START_NOT_STICKY
        // Android 14+: the foreground service must exist before the projection is created.
        val channel = NotificationChannel("mirror_spike", "Mirror spike", NotificationManager.IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        val notification = Notification.Builder(this, "mirror_spike")
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Mirror spike: sharing screen")
            .setOngoing(true)
            .build()
        val type = if (Build.VERSION.SDK_INT >= 29) ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION else 0
        ServiceCompat.startForeground(this, 7, notification, type)

        val resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, 0)
        val data = IntentCompat.getParcelableExtra(intent, EXTRA_DATA, Intent::class.java)
        val host = intent.getStringExtra(EXTRA_HOST).orEmpty()
        val settings = Settings(
            mbps = intent.getIntExtra(EXTRA_MBPS, 30),
            maxSide = intent.getIntExtra(EXTRA_MAX_SIDE, 2304),
            hevc = intent.getBooleanExtra(EXTRA_HEVC, false),
            paceMbps = intent.getIntExtra(EXTRA_PACE_MBPS, 200),
            intraRefresh = intent.getBooleanExtra(EXTRA_INTRA_REFRESH, false),
            speedups = intent.getBooleanExtra(EXTRA_SPEEDUPS, false),
        )
        if (data == null || host.isEmpty()) {
            status = "Missing PC address"
            stopSelf()
            return START_NOT_STICKY
        }

        val mp = getSystemService(MediaProjectionManager::class.java).getMediaProjection(resultCode, data)
        if (mp == null) {
            status = "Screen capture refused"
            stopSelf()
            return START_NOT_STICKY
        }
        projection = mp
        // Android 14+ requires a callback before createVirtualDisplay.
        mp.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                stopSelf()
            }
        }, Handler(Looper.getMainLooper()))

        running = true
        thread(name = "mirror-spike") {
            try {
                stream(mp, host, settings)
            } catch (e: Exception) {
                Log.e(TAG, "stream failed", e)
                status = "Error: ${e.message}"
            }
            Handler(Looper.getMainLooper()).post { stopSelf() }
        }
        return START_NOT_STICKY
    }

    private data class Settings(
        val mbps: Int,
        val maxSide: Int,
        val hevc: Boolean,
        /** Sending rate cap in Mbit/s; 0 = send each frame as one burst. */
        val paceMbps: Int,
        /** Spread intra-coded blocks over frames instead of sending big keyframes. */
        val intraRefresh: Boolean,
        /** Qualcomm low-latency switch, highest clock, one frame in flight. */
        val speedups: Boolean,
    )

    private fun screenSize(): Pair<Int, Int> {
        val wm = getSystemService(WindowManager::class.java)
        if (Build.VERSION.SDK_INT >= 30) {
            val b = wm.maximumWindowMetrics.bounds
            return b.width() to b.height()
        }
        val m = android.util.DisplayMetrics()
        @Suppress("DEPRECATION")
        wm.defaultDisplay.getRealMetrics(m)
        return m.widthPixels to m.heightPixels
    }

    /** The encoder, and whether intra-refresh is really on. */
    private fun createEncoder(
        w: Int, h: Int, s: Settings, withProfile: Boolean, speedups: Boolean,
    ): Pair<MediaCodec, Boolean> {
        val mime = if (s.hevc) MediaFormat.MIMETYPE_VIDEO_HEVC else MediaFormat.MIMETYPE_VIDEO_AVC
        val c = MediaCodec.createEncoderByType(mime)
        val codecCaps = c.codecInfo.getCapabilitiesForType(mime)
        val caps = codecCaps.encoderCapabilities
        val cbr = caps.isBitrateModeSupported(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
        val refresh = s.intraRefresh &&
            codecCaps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_IntraRefresh)
        val vendor = if (Build.VERSION.SDK_INT >= 31) c.supportedVendorParameters else null
        val qti = speedups && (vendor == null || QTI_LOW_LATENCY in vendor)
        val format = MediaFormat.createVideoFormat(mime, w, h).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, s.mbps * 1_000_000)
            // Constant bitrate: in variable mode the encoder starved busy scenes.
            if (cbr) setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            setInteger(MediaFormat.KEY_FRAME_RATE, 60)
            if (refresh) {
                // No periodic keyframes: every frame refreshes a slice of the picture.
                setInteger(MediaFormat.KEY_INTRA_REFRESH_PERIOD, REFRESH_FRAMES)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 3600)
            } else {
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10) // plus on demand after a loss
            }
            setInteger(MediaFormat.KEY_PRIORITY, 0) // real time
            // Keep sending ~10 fps when the screen is still: the PC sees a live
            // stream, and the encoder sharpens the still picture.
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000)
            if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            if (Build.VERSION.SDK_INT >= 29) setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            if (speedups) {
                // Run the encoder at its highest clock, and hand out each frame as
                // soon as it's encoded.
                setInteger(MediaFormat.KEY_OPERATING_RATE, Short.MAX_VALUE.toInt())
                if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LATENCY, 1)
                if (qti) setInteger(QTI_LOW_LATENCY, 1)
            }
            // Say exactly which colours we mean, so the PC converts them back correctly.
            setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709)
            setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED)
            setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO)
            if (withProfile) {
                if (s.hevc) {
                    setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.HEVCProfileMain)
                } else {
                    // High: noticeably sharper than Baseline at the same bitrate.
                    setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileHigh)
                    setInteger(MediaFormat.KEY_LEVEL, MediaCodecInfo.CodecProfileLevel.AVCLevel51)
                }
            }
        }
        try {
            c.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        } catch (e: Exception) {
            c.release()
            throw e
        }

        info.clear()
        info += "encoder ${c.name}, ${w}x$h, ${s.mbps} Mbit/s " + (if (cbr) "CBR" else "VBR") +
            (if (withProfile) "" else ", default profile")
        info += "pacing " + (if (s.paceMbps > 0) "${s.paceMbps} Mbit/s" else "off") +
            ", intra-refresh " + when {
                refresh -> "on ($REFRESH_FRAMES frames)"
                s.intraRefresh -> "NOT SUPPORTED by this encoder"
                else -> "off"
            }
        info += "speed-ups " + when {
            !s.speedups -> "off"
            !speedups -> "REFUSED by the encoder (running without)"
            else -> "on (max clock, latency 1" + (if (qti) ", $QTI_LOW_LATENCY" else ", no Qualcomm low-latency key") + ")"
        }
        if (vendor != null) {
            val names = vendor.filter { "latency" in it || "refresh" in it || "perf" in it || "ltr" in it }.sorted()
            info += "vendor keys about latency/refresh/perf/LTR: " + names.joinToString().ifEmpty { "none" }
        }
        return c to refresh
    }

    /**
     * Keeps the phone's Wi-Fi out of power saving while mirroring. Otherwise it
     * naps between packets and wakes on the router's schedule (every ~100 ms
     * beacon), a classic cause of random 20-100 ms stalls. The low-latency
     * mode (Android 10+) applies while the screen is on, which it is while
     * mirroring; the older high-performance lock is held too, for phones that
     * only honour that one.
     */
    private fun holdWifi() {
        val wm = applicationContext.getSystemService(WifiManager::class.java) ?: return
        val modes = buildList {
            if (Build.VERSION.SDK_INT >= 29) add(WifiManager.WIFI_MODE_FULL_LOW_LATENCY to "low-latency")
            @Suppress("DEPRECATION")
            add(WifiManager.WIFI_MODE_FULL_HIGH_PERF to "high-performance")
        }
        val held = modes.mapNotNull { (mode, name) ->
            try {
                wm.createWifiLock(mode, "nudge:mirror-$name").apply {
                    setReferenceCounted(false)
                    acquire()
                    wifiLocks += this
                }
                name
            } catch (e: Exception) {
                Log.w(TAG, "Wi-Fi lock $name refused", e)
                null
            }
        }
        info += "Wi-Fi power saving off, locks held: " + held.joinToString().ifEmpty { "none" }
    }

    private fun sendInfo(sock: DatagramSocket, pc: SocketAddress) {
        for (line in info) {
            val bytes = line.toByteArray().let { if (it.size > CHUNK) it.copyOf(CHUNK) else it }
            try {
                sock.send(DatagramPacket(byteArrayOf(T_INFO) + bytes, bytes.size + 1, pc))
            } catch (_: Exception) {
            }
        }
    }

    private fun stream(mp: MediaProjection, host: String, s: Settings) {
        val (sw, sh) = screenSize()
        val scale = minOf(1.0, s.maxSide.toDouble() / maxOf(sw, sh))
        val w = (sw * scale).toInt() / 8 * 8
        val h = (sh * scale).toInt() / 8 * 8

        // Fall back step by step if the encoder refuses a setting.
        val attempts = listOf(true to s.speedups, true to false, false to false).distinct()
        var created: Pair<MediaCodec, Boolean>? = null
        for ((withProfile, speedups) in attempts) {
            created = try {
                createEncoder(w, h, s, withProfile, speedups)
            } catch (e: Exception) {
                Log.w(TAG, "encoder refused profile=$withProfile speedups=$speedups", e)
                if ((withProfile to speedups) == attempts.last()) throw e
                continue
            }
            break
        }
        val (c, refresh) = created!!
        codec = c
        val surface = c.createInputSurface()
        c.start()
        if (s.speedups && Build.VERSION.SDK_INT >= 31 && QTI_LOW_LATENCY in c.supportedVendorParameters) {
            // Some codecs only take vendor keys after start.
            try {
                c.setParameters(Bundle().apply { putInt(QTI_LOW_LATENCY, 1) })
            } catch (_: Exception) {
            }
        }
        display = mp.createVirtualDisplay(
            "nudge-mirror-spike", w, h, resources.displayMetrics.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, null,
        )

        val sock = DatagramSocket().apply {
            sendBufferSize = 4 shl 20
            soTimeout = 500
        }
        socket = sock
        val pc = InetSocketAddress(host, PORT)
        sock.send(DatagramPacket(byteArrayOf(T_HELLO), 1, pc))
        holdWifi()
        sendInfo(sock, pc)
        thread(name = "mirror-spike-control") { control(sock, c, pc) }

        val codecName = if (s.hevc) "H.265" else "H.264"
        status = "Streaming $codecName ${w}x$h at ${s.mbps} Mbit/s to $host"
        val bufInfo = MediaCodec.BufferInfo()
        var csd: ByteArray? = null
        var frameId = 0
        var sent = 0
        var bytes = 0L
        var encodeUs = 0L
        var windowStart = System.nanoTime()
        val header = ByteBuffer.allocate(HEADER).order(ByteOrder.LITTLE_ENDIAN)
        // Pacing: the time the next packet may go out at the capped rate.
        val nsPerByte = if (s.paceMbps > 0) 8_000.0 / s.paceMbps else 0.0
        var nextSendNs = 0L

        while (running) {
            val index = try {
                c.dequeueOutputBuffer(bufInfo, 10_000)
            } catch (e: IllegalStateException) {
                break // stopped from another thread
            }
            if (index < 0) continue
            val encodedUs = System.nanoTime() / 1000
            val out = c.getOutputBuffer(index)!!
            val bytesOut = ByteArray(bufInfo.size)
            out.position(bufInfo.offset)
            out.get(bytesOut)
            c.releaseOutputBuffer(index, false)

            if (bufInfo.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                csd = bytesOut // SPS/PPS: sent in front of every keyframe
                continue
            }
            val key = bufInfo.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
            val payload = if (key && csd != null) csd + bytesOut else bytesOut
            // For surface input, the timestamp is when the compositor produced the frame.
            val captureUs = bufInfo.presentationTimeUs
            val flags = (if (key) F_KEY else 0) or (if (s.hevc) F_HEVC else 0) or
                (if (refresh) F_INTRA_REFRESH else 0)

            val count = (payload.size + CHUNK - 1) / CHUNK
            val packets = Array(count) { i ->
                val from = i * CHUNK
                val len = minOf(CHUNK, payload.size - from)
                header.clear()
                header.put(T_FRAG).putInt(frameId).putShort(i.toShort()).putShort(count.toShort())
                    .put(flags.toByte()).putLong(captureUs).putLong(encodedUs)
                val p = ByteArray(HEADER + len)
                System.arraycopy(header.array(), 0, p, 0, HEADER)
                System.arraycopy(payload, from, p, HEADER, len)
                p
            }
            synchronized(history) { history[frameId] = packets }
            for (p in packets) {
                if (nsPerByte > 0) {
                    // Allow up to 1 ms of burst, then hold back to the paced rate.
                    val now = System.nanoTime()
                    nextSendNs = maxOf(nextSendNs, now) + (p.size * nsPerByte).toLong()
                    val ahead = nextSendNs - now - 1_000_000
                    if (ahead > 0) LockSupport.parkNanos(ahead)
                }
                sock.send(DatagramPacket(p, p.size, pc))
            }
            frameId++
            sent++
            bytes += payload.size
            encodeUs += encodedUs - captureUs

            val now = System.nanoTime()
            if (now - windowStart >= 1_000_000_000L) {
                val secs = (now - windowStart) / 1e9
                status = "Streaming $codecName ${w}x$h to $host\n${info.take(3).joinToString("\n")}\n" +
                    "%.0f fps, %.1f Mbit/s, encode %.1f ms avg\nresends: %d NACKs, %d packets; keyframe requests %d".format(
                        sent / secs, bytes * 8 / secs / 1e6, if (sent > 0) encodeUs / 1000.0 / sent else 0.0,
                        nacks, resent, keyRequests,
                    )
                sent = 0
                bytes = 0
                encodeUs = 0
                windowStart = now
            }
        }
    }

    /** Answers the PC's clock pings, keyframe requests and NACKs. */
    private fun control(sock: DatagramSocket, c: MediaCodec, pc: SocketAddress) {
        val buf = ByteArray(2048)
        val reply = ByteBuffer.allocate(17).order(ByteOrder.LITTLE_ENDIAN)
        val le = ByteBuffer.wrap(buf).order(ByteOrder.LITTLE_ENDIAN)
        var pings = 0
        while (running) {
            val p = DatagramPacket(buf, buf.size)
            try {
                sock.receive(p)
            } catch (e: SocketTimeoutException) {
                continue
            } catch (e: Exception) {
                return
            }
            when (buf[0]) {
                T_PING -> if (p.length >= 9) {
                    val phoneUs = System.nanoTime() / 1000
                    reply.clear()
                    reply.put(T_PONG).put(buf, 1, 8).putLong(phoneUs)
                    try {
                        sock.send(DatagramPacket(reply.array(), 17, p.socketAddress))
                    } catch (_: Exception) {
                    }
                    // Repeat the settings now and then, in case the first copy was lost.
                    if (++pings % 50 == 1) sendInfo(sock, pc)
                }
                T_KEY -> try {
                    keyRequests++
                    c.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
                } catch (_: Exception) {
                }
                T_NACK -> if (p.length >= 7) {
                    nacks++
                    val id = le.getInt(1)
                    val n = le.getShort(5).toInt() and 0xFFFF
                    val packets = synchronized(history) { history[id] } ?: continue
                    val wanted = if (n == 0xFFFF) packets.indices.toList()
                    else (0 until minOf(n, (p.length - 7) / 2)).map { le.getShort(7 + it * 2).toInt() and 0xFFFF }
                    for (i in wanted) {
                        val orig = packets.getOrNull(i) ?: continue
                        val copy = orig.copyOf()
                        copy[9] = (copy[9].toInt() or F_RESENT).toByte()
                        try {
                            sock.send(DatagramPacket(copy, copy.size, p.socketAddress))
                            resent++
                        } catch (_: Exception) {
                        }
                    }
                }
            }
        }
    }

    override fun onDestroy() {
        running = false
        wifiLocks.forEach { if (it.isHeld) it.release() }
        wifiLocks.clear()
        display?.release()
        try {
            codec?.stop()
        } catch (_: Exception) {
        }
        codec?.release()
        projection?.stop()
        socket?.close()
        status = "Stopped"
        super.onDestroy()
    }
}
