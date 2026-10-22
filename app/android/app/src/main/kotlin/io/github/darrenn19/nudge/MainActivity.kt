package io.github.darrenn19.nudge

import android.Manifest
import android.annotation.SuppressLint
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.util.concurrent.Executors

/**
 * The window. Uses the engine [NudgeApplication] started, so closing the
 * window doesn't stop transfers.
 *
 * Also where files come in to be sent: from the share sheet, the quick
 * settings tile, or the in-app picker. They wait in [incoming] until Dart
 * collects them with `takeShared`.
 */
class MainActivity : FlutterActivity() {
    private var channel: MethodChannel? = null
    private var pendingPick: MethodChannel.Result? = null
    private val incoming = mutableListOf<Map<String, Any>>()
    private val worker = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())

    override fun getCachedEngineId(): String = NudgeApplication.ENGINE_ID

    override fun shouldDestroyEngineWithHost(): Boolean = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) handle(intent)
        askForNotifications()
        // From the foreground, Android always allows this.
        NudgeService.start(this)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL).apply {
            setMethodCallHandler { call, result ->
                when (call.method) {
                    "takeShared" -> {
                        result.success(incoming.toList())
                        incoming.clear()
                    }
                    "pickFiles" -> pick(result)
                    "moveToBack" -> {
                        moveTaskToBack(true)
                        result.success(null)
                    }
                    "askToRunInBackground" -> {
                        askToRunInBackground()
                        result.success(null)
                    }
                    else -> result.notImplemented()
                }
            }
        }
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        channel?.setMethodCallHandler(null)
        channel = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    /** Files shared with Nudge, or a request from the tile to pick some. */
    private fun handle(intent: Intent?) {
        if (intent == null || intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return
        if (intent.action in SEND_ACTIONS) {
            // Lets Dart open a connection while the files are still being
            // opened (or picked), so sending starts the moment they're ready.
            channel?.invokeMethod("shareStarting", null)
        }
        when (intent.action) {
            Intent.ACTION_SEND -> {
                val uri = intent.stream()
                val text = intent.getStringExtra(Intent.EXTRA_TEXT)
                when {
                    uri != null -> open(listOf(uri))
                    !text.isNullOrBlank() -> worker.execute { deliver(listOf(SharedFiles.text(this, text))) }
                }
            }
            Intent.ACTION_SEND_MULTIPLE -> open(intent.streams())
            ACTION_PICK_FILES -> pick(null)
        }
        // Don't handle the same share twice if the activity is recreated.
        setIntent(Intent(this, MainActivity::class.java))
    }

    /** Opening can mean copying (see [SharedFiles]), so it happens off the UI thread. */
    private fun open(uris: List<Uri>) {
        if (uris.isEmpty()) return
        worker.execute { deliver(uris.mapNotNull { SharedFiles.open(this, it) }) }
    }

    private fun deliver(files: List<Map<String, Any>>) = main.post {
        if (files.isEmpty()) return@post
        incoming.addAll(files)
        channel?.invokeMethod("sharedAvailable", null)
    }

    /** Pick files to send. With a [result], they go back to that call; otherwise they arrive like a share. */
    private fun pick(result: MethodChannel.Result?) {
        pendingPick?.success(emptyList<Any>())
        pendingPick = result
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("*/*")
            .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        @Suppress("DEPRECATION") // FlutterActivity isn't a ComponentActivity.
        startActivityForResult(intent, PICK_REQUEST)
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != PICK_REQUEST) return
        val result = pendingPick
        pendingPick = null
        val uris = buildList {
            data?.clipData?.let { clip -> for (i in 0 until clip.itemCount) add(clip.getItemAt(i).uri) }
            if (isEmpty()) data?.data?.let(::add)
        }
        if (result == null) {
            open(uris)
            return
        }
        worker.execute {
            val files = uris.mapNotNull { SharedFiles.open(this, it) }
            main.post { result.success(files) }
        }
    }

    /** Android 13+ hides notifications until the user allows them. Ask once. */
    private fun askForNotifications() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) return
        val prefs = getSharedPreferences("nudge", MODE_PRIVATE)
        if (prefs.getBoolean("askedNotifications", false)) return
        prefs.edit().putBoolean("askedNotifications", true).apply()
        requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
    }

    /**
     * Ask once to be left out of battery optimization. Without it, Samsung
     * and others stop Nudge in the background, which kills long transfers
     * and receiving while the app is closed. It also lets a transfer that
     * arrives in the background start its foreground service.
     */
    @SuppressLint("BatteryLife") // Running in the background is the app's job.
    private fun askToRunInBackground() {
        val power = getSystemService(PowerManager::class.java) ?: return
        if (power.isIgnoringBatteryOptimizations(packageName)) return
        val prefs = getSharedPreferences("nudge", MODE_PRIVATE)
        if (prefs.getBoolean("askedBattery", false)) return
        prefs.edit().putBoolean("askedBattery", true).apply()
        val ask = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
        runCatching { startActivity(ask) }.onFailure {
            // Some builds hide the direct prompt; the settings list still works.
            runCatching { startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
        }
    }

    override fun onDestroy() {
        worker.shutdown()
        super.onDestroy()
    }

    private fun Intent.stream(): Uri? =
        if (Build.VERSION.SDK_INT >= 33) {
            getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            @Suppress("DEPRECATION")
            getParcelableExtra(Intent.EXTRA_STREAM)
        }

    private fun Intent.streams(): List<Uri> =
        if (Build.VERSION.SDK_INT >= 33) {
            getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            @Suppress("DEPRECATION")
            getParcelableArrayListExtra(Intent.EXTRA_STREAM)
        }.orEmpty()

    companion object {
        const val CHANNEL = "nudge/activity"
        const val ACTION_PICK_FILES = "io.github.darrenn19.nudge.PICK_FILES"
        private const val PICK_REQUEST = 4201
        private val SEND_ACTIONS = setOf(Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE, ACTION_PICK_FILES)
    }
}
