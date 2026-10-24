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
 * The window. Its Flutter engine lives only while it's open: the core runs
 * without it (see [NudgeApplication]), so closing the window frees the
 * memory and transfers carry on.
 *
 * Files come here to have a device chosen: from [ShareActivity] when there
 * was a choice, from the quick settings tile, or the in-app picker. They
 * wait in [Inbox] until Dart collects them with `takeShared`.
 */
class MainActivity : FlutterActivity() {
    private var channel: MethodChannel? = null
    private var platform: MethodChannel? = null
    private var pendingPick: MethodChannel.Result? = null
    private var pendingFolder: MethodChannel.Result? = null
    private val worker = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())

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

    override fun onResume() {
        super.onResume()
        Background.inFront = true
    }

    override fun onPause() {
        Background.inFront = false
        super.onPause()
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        platform = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, PlatformChannel.NAME)
            .apply { setMethodCallHandler(PlatformChannel(applicationContext)) }
        channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL).apply {
            setMethodCallHandler { call, result ->
                when (call.method) {
                    "takeShared" -> result.success(Inbox.take())
                    "pickFiles" -> pick(result)
                    "pickFolder" -> pickFolder(result)
                    "moveToBack" -> {
                        moveTaskToBack(true)
                        result.success(null)
                    }
                    "askToRunInBackground" -> {
                        askToRunInBackground()
                        result.success(null)
                    }
                    "askForDirectLink" -> {
                        askForDirectLink()
                        result.success(null)
                    }
                    // Samsung puts apps to sleep even with the battery
                    // exemption; only "Never sleeping apps" keeps them awake.
                    "needsSleepGuide" -> {
                        val prefs = getSharedPreferences("nudge", MODE_PRIVATE)
                        val samsung = Build.MANUFACTURER.equals("samsung", ignoreCase = true)
                        result.success(samsung && !prefs.getBoolean("sleepGuided", false))
                    }
                    "openSleepSettings" -> {
                        getSharedPreferences("nudge", MODE_PRIVATE).edit().putBoolean("sleepGuided", true).apply()
                        openSleepSettings()
                        result.success(null)
                    }
                    "sleepGuideDismissed" -> {
                        getSharedPreferences("nudge", MODE_PRIVATE).edit().putBoolean("sleepGuided", true).apply()
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
        platform?.setMethodCallHandler(null)
        platform = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    /** Files waiting for a device ([ShareActivity]), or a request from the tile to pick some. */
    private fun handle(intent: Intent?) {
        if (intent == null || intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return
        when (intent.action) {
            ACTION_SHARED -> channel?.invokeMethod("sharedAvailable", null)
            // From the "Send latest" tile: photo access first, or a device
            // to choose.
            ACTION_ASK_PHOTOS -> requestPermissions(Latest.permissions, PHOTOS_REQUEST)
            ACTION_SEND_LATEST -> latestToWindow()
            ACTION_PICK_FILES -> {
                // Lets Dart open connections while the user picks.
                channel?.invokeMethod("shareStarting", null)
                pick(null)
            }
        }
        // Don't handle the same request twice if the activity is recreated.
        setIntent(Intent(this, MainActivity::class.java))
    }

    /** Opening can mean copying (see [SharedFiles]), so it happens off the UI thread. */
    private fun open(uris: List<Uri>) {
        if (uris.isEmpty()) return
        worker.execute {
            val files = uris.mapNotNull { SharedFiles.open(this, it) }
            main.post {
                if (files.isEmpty()) return@post
                Inbox.add(files)
                channel?.invokeMethod("sharedAvailable", null)
            }
        }
    }

    /** The newest photo or video, waiting for a device to be chosen here. */
    private fun latestToWindow() = worker.execute {
        val file = Latest.find(this)?.let { SharedFiles.open(this, it.uri) }
        main.post {
            if (file == null) {
                android.widget.Toast.makeText(this, "No photo or download to send", android.widget.Toast.LENGTH_SHORT).show()
                return@post
            }
            Inbox.add(listOf(file))
            channel?.invokeMethod("sharedAvailable", null)
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != PHOTOS_REQUEST || grantResults.none { it == PackageManager.PERMISSION_GRANTED }) return
        // Allowed: do what the tile was tapped for.
        worker.execute {
            when (Latest.send(this)) {
                Latest.Result.SENT -> main.post { moveTaskToBack(true) }
                Latest.Result.CHOOSE -> latestToWindow()
                Latest.Result.NONE -> Latest.say(this, "No photo or download to send")
            }
        }
    }

    /** Pick a folder to send: its files go back to [result] (see [Folders]). */
    private fun pickFolder(result: MethodChannel.Result) {
        pendingFolder?.success(emptyList<Any>())
        pendingFolder = result
        @Suppress("DEPRECATION") // FlutterActivity isn't a ComponentActivity.
        startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), FOLDER_REQUEST)
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
        if (requestCode == FOLDER_REQUEST) {
            val result = pendingFolder
            pendingFolder = null
            val tree = data?.data
            if (result == null || tree == null) {
                result?.success(emptyList<Any>())
                return
            }
            // Walking and opening every file can take a moment.
            worker.execute {
                val files = runCatching { Folders.open(this, tree) }.getOrDefault(emptyList())
                main.post { result.success(files) }
            }
            return
        }
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

    /**
     * The hotspot for big transfers needs "nearby devices" (Android 13+) or
     * location (older). Asked once; without it transfers just stay on the
     * usual Wi-Fi.
     */
    private fun askForDirectLink() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || Hotspot.allowed(this)) return
        val prefs = getSharedPreferences("nudge", MODE_PRIVATE)
        if (prefs.getBoolean("askedDirectLink", false)) return
        prefs.edit().putBoolean("askedDirectLink", true).apply()
        val permission = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            Manifest.permission.NEARBY_WIFI_DEVICES
        } else {
            Manifest.permission.ACCESS_FINE_LOCATION
        }
        requestPermissions(arrayOf(permission), 2)
    }

    /**
     * Samsung's Settings → Battery → Background usage limits, where "Never
     * sleeping apps" lives; the app's own settings page if that isn't there.
     */
    private fun openSleepSettings() {
        val samsung = Intent("com.samsung.android.sm.ACTION_START_APP_POWER_MANAGEMENT_SETTING")
        runCatching { startActivity(samsung) }.onFailure {
            runCatching {
                startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName")))
            }
        }
    }

    override fun onDestroy() {
        worker.shutdown()
        super.onDestroy()
    }

    companion object {
        const val CHANNEL = "nudge/activity"
        const val ACTION_PICK_FILES = "io.github.darrenn19.nudge.PICK_FILES"
        /** Files are waiting in [Inbox] for a device to be chosen. */
        const val ACTION_SHARED = "io.github.darrenn19.nudge.SHARED"
        const val ACTION_ASK_PHOTOS = "io.github.darrenn19.nudge.ASK_PHOTOS"
        const val ACTION_SEND_LATEST = "io.github.darrenn19.nudge.SEND_LATEST"
        private const val PHOTOS_REQUEST = 3
        private const val PICK_REQUEST = 4201
        private const val FOLDER_REQUEST = 4202
    }
}
