package io.github.darrenn19.nudge.spike

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.media.projection.MediaProjectionManager
import android.os.Bundle
import android.text.InputType
import android.view.Choreographer
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Screen-mirroring latency spike (throwaway, debug builds only).
 *
 * Shows a clock that changes every frame (the screen must keep changing, or
 * the capture sends nothing), plus fields for the PC address and encoder settings.
 */
class MirrorSpikeActivity : Activity() {
    private lateinit var host: EditText
    private lateinit var mbps: EditText
    private lateinit var maxSide: EditText
    private lateinit var hevc: CheckBox
    private lateinit var pace: EditText
    private lateinit var intraRefresh: CheckBox
    private lateinit var speedups: CheckBox
    private lateinit var clock: TextView
    private lateinit var bar: View
    private lateinit var status: TextView
    private var ticking = false

    private val tick = object : Choreographer.FrameCallback {
        override fun doFrame(frameTimeNanos: Long) {
            if (!ticking) return
            val ms = System.nanoTime() / 1_000_000
            clock.text = "%05d".format(ms % 100_000)
            // A bar sweeping across once a second makes motion easy to judge by eye.
            val width = (bar.parent as View).width
            bar.translationX = ((ms % 1000) / 1000f) * (width - bar.width)
            status.text = MirrorSpikeService.status
            Choreographer.getInstance().postFrameCallback(this)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val prefs = getSharedPreferences("mirror_spike", Context.MODE_PRIVATE)
        val pad = (16 * resources.displayMetrics.density).toInt()

        fun field(hint: String, value: String, type: Int) = EditText(this).apply {
            this.hint = hint
            setText(value)
            inputType = type
        }
        host = field("PC address, e.g. 192.168.1.20", prefs.getString("host", "")!!, InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        mbps = field("Bitrate (Mbit/s)", prefs.getString("mbps30", "30")!!, InputType.TYPE_CLASS_NUMBER)
        maxSide = field("Longest side (px)", prefs.getString("max2304", "2304")!!, InputType.TYPE_CLASS_NUMBER)
        pace = field("Pacing cap (Mbit/s, 0 = off)", prefs.getString("pace", "200")!!, InputType.TYPE_CLASS_NUMBER)

        hevc = CheckBox(this).apply {
            text = "H.265 (sharper at the same bitrate)"
            isChecked = prefs.getBoolean("hevc", false)
        }
        intraRefresh = CheckBox(this).apply {
            text = "Intra-refresh (no big keyframes)"
            isChecked = prefs.getBoolean("intraRefresh", true)
        }
        speedups = CheckBox(this).apply {
            text = "Encoder speed-ups (Qualcomm low-latency, max clock)"
            isChecked = prefs.getBoolean("speedups", true)
        }

        clock = TextView(this).apply {
            textSize = 72f
            typeface = Typeface.MONOSPACE
            gravity = Gravity.CENTER
        }
        val track = LinearLayout(this).apply { minimumHeight = pad * 3 }
        bar = View(this).apply { setBackgroundColor(Color.rgb(230, 80, 40)) }
        track.addView(bar, LinearLayout.LayoutParams(pad * 3, pad * 3))
        status = TextView(this).apply { text = MirrorSpikeService.status }

        // For unattended runs from the PC: settings as extras, then Start.
        // adb shell am start -n <pkg>/.spike.MirrorSpikeActivity --es host 10.0.0.2 --ez auto true ...
        intent?.let { i ->
            i.getStringExtra("host")?.let { host.setText(it) }
            i.getStringExtra("mbps")?.let { mbps.setText(it) }
            i.getStringExtra("maxSide")?.let { maxSide.setText(it) }
            i.getStringExtra("pace")?.let { pace.setText(it) }
            if (i.hasExtra("hevc")) hevc.isChecked = i.getBooleanExtra("hevc", false)
            if (i.hasExtra("intraRefresh")) intraRefresh.isChecked = i.getBooleanExtra("intraRefresh", false)
            if (i.hasExtra("speedups")) speedups.isChecked = i.getBooleanExtra("speedups", false)
        }
        val start = Button(this).apply {
            text = "Start"
            setOnClickListener {
                prefs.edit()
                    .putString("host", host.text.toString().trim())
                    .putString("mbps30", mbps.text.toString())
                    .putString("max2304", maxSide.text.toString())
                    .putBoolean("hevc", hevc.isChecked)
                    .putString("pace", pace.text.toString())
                    .putBoolean("intraRefresh", intraRefresh.isChecked)
                    .putBoolean("speedups", speedups.isChecked)
                    .apply()
                val mpm = getSystemService(MediaProjectionManager::class.java)
                @Suppress("DEPRECATION")
                startActivityForResult(mpm.createScreenCaptureIntent(), REQUEST_CAPTURE)
            }
        }
        val stop = Button(this).apply {
            text = "Stop"
            setOnClickListener { stopService(Intent(this@MirrorSpikeActivity, MirrorSpikeService::class.java)) }
        }

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad * 3, pad, pad)
            addView(TextView(this@MirrorSpikeActivity).apply {
                text = "Nudge mirror latency spike"
                textSize = 20f
            })
            addView(host)
            addView(mbps)
            addView(maxSide)
            addView(pace)
            addView(hevc)
            addView(intraRefresh)
            addView(speedups)
            addView(LinearLayout(this@MirrorSpikeActivity).apply {
                addView(start)
                addView(stop)
            })
            addView(clock)
            addView(track)
            addView(status)
        }
        setContentView(root)
        // A direct link for testing without the router: the phone hosts a
        // network (as for big transfers) and logs how to join it.
        // adb logcat -s MirrorSpike   (look for "hotspot")
        if (intent?.getBooleanExtra("hotspot", false) == true) {
            io.github.darrenn19.nudge.Hotspot.start(this) { h ->
                val text = h?.let { "hotspot ssid=${it["ssid"]} pass=${it["passphrase"]} mhz=${it["mhz"]}" } ?: "hotspot failed"
                android.util.Log.i("MirrorSpike", text)
                status.text = text
            }
        }
        if (intent?.getBooleanExtra("hotspotStop", false) == true) io.github.darrenn19.nudge.Hotspot.stop()
        if (intent?.getBooleanExtra("auto", false) == true) root.post { start.performClick() }
    }

    override fun onResume() {
        super.onResume()
        ticking = true
        Choreographer.getInstance().postFrameCallback(tick)
    }

    override fun onPause() {
        ticking = false
        super.onPause()
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_CAPTURE || resultCode != RESULT_OK || data == null) return
        val intent = Intent(this, MirrorSpikeService::class.java)
            .putExtra(MirrorSpikeService.EXTRA_RESULT_CODE, resultCode)
            .putExtra(MirrorSpikeService.EXTRA_DATA, data)
            .putExtra(MirrorSpikeService.EXTRA_HOST, host.text.toString().trim())
            .putExtra(MirrorSpikeService.EXTRA_MBPS, mbps.text.toString().toIntOrNull() ?: 30)
            .putExtra(MirrorSpikeService.EXTRA_MAX_SIDE, maxSide.text.toString().toIntOrNull() ?: 2304)
            .putExtra(MirrorSpikeService.EXTRA_HEVC, hevc.isChecked)
            .putExtra(MirrorSpikeService.EXTRA_PACE_MBPS, pace.text.toString().toIntOrNull() ?: 200)
            .putExtra(MirrorSpikeService.EXTRA_INTRA_REFRESH, intraRefresh.isChecked)
            .putExtra(MirrorSpikeService.EXTRA_SPEEDUPS, speedups.isChecked)
        startForegroundService(intent)
    }

    private companion object {
        const val REQUEST_CAPTURE = 4711
    }
}
