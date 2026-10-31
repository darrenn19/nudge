package io.github.darrenn19.nudge

import android.annotation.SuppressLint
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.projection.MediaProjection
import android.os.SystemClock
import android.util.Log
import kotlin.concurrent.thread

/**
 * This phone's sound, for a PC that asked to play it (screen mirroring's
 * "Play phone audio on this PC"): what apps play, captured through the
 * screen-sharing session's [MediaProjection] (Android 10+, needs
 * RECORD_AUDIO), sent as raw PCM ([Core.mirrorSendAudio]: 48 kHz, stereo,
 * 16-bit, 10 ms at a time; `nudge_core::input::AUDIO_STREAM`).
 *
 * Apps that opt out of capture (some calls, DRM video) stay silent; Android
 * decides that.
 */
class AudioCapture(private val projection: MediaProjection, private val session: Long) {
    companion object {
        private const val TAG = "Nudge"
        private const val RATE = 48_000
        private const val CHUNK = RATE / 100 * 4 // 10 ms of stereo 16-bit
    }

    @Volatile
    private var stopped = false

    /** False if Android refused (no permission, or capture unsupported). */
    @SuppressLint("MissingPermission") // checked by the caller
    fun start(): Boolean {
        val record = try {
            val config = AudioPlaybackCaptureConfiguration.Builder(projection)
                .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                .addMatchingUsage(AudioAttributes.USAGE_GAME)
                .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                .build()
            val format = AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(RATE)
                .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                .build()
            val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT)
            AudioRecord.Builder()
                .setAudioFormat(format)
                .setBufferSizeInBytes(maxOf(min, CHUNK * 8))
                .setAudioPlaybackCaptureConfig(config)
                .build()
        } catch (e: Exception) {
            Log.w(TAG, "mirror audio: can't capture", e)
            return false
        }
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            Log.w(TAG, "mirror audio: capture didn't start")
            record.release()
            return false
        }
        thread(name = "nudge-mirror-audio") {
            android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_URGENT_AUDIO)
            val buf = ByteArray(CHUNK)
            var peak = 0
            var chunks = 0
            var since = SystemClock.uptimeMillis()
            try {
                record.startRecording()
                Log.i(TAG, "mirror audio: capturing")
                while (!stopped) {
                    val n = record.read(buf, 0, CHUNK)
                    if (n <= 0) {
                        if (n < 0) Log.w(TAG, "mirror audio: read failed ($n)")
                        if (n < 0) break
                        continue
                    }
                    if (Core.mirrorSendAudio(session, buf, n) < 0) break
                    // Loudest sample, for the log: 0 means silence arrives.
                    var i = 0
                    while (i + 1 < n) {
                        val v = (buf[i].toInt() and 0xFF) or (buf[i + 1].toInt() shl 8)
                        peak = maxOf(peak, Math.abs(v.toShort().toInt()))
                        i += 2
                    }
                    chunks++
                    val now = SystemClock.uptimeMillis()
                    if (now - since >= 10_000) {
                        Log.i(TAG, "mirror audio: $chunks chunks in 10 s, peak ${peak * 100 / 32768}%")
                        peak = 0
                        chunks = 0
                        since = now
                    }
                }
            } catch (e: Exception) {
                Log.w(TAG, "mirror audio: stopped", e)
            } finally {
                runCatching { record.stop() }
                record.release()
                Log.i(TAG, "mirror audio: off")
            }
        }
        return true
    }

    fun stop() {
        stopped = true
    }
}
