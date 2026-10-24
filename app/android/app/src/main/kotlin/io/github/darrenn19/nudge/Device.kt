package io.github.darrenn19.nudge

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.provider.Settings
import java.io.File

/** This phone's name and folders, as the core and the app both see them. */
object Device {
    /** The name the user gave the phone (Settings → About phone), else the model. */
    fun name(context: Context): String {
        val named = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
        if (!named.isNullOrBlank()) return named
        val model = Build.MODEL ?: "Android phone"
        val maker = Build.MANUFACTURER ?: ""
        return if (model.startsWith(maker, ignoreCase = true)) model else "$maker $model".trim()
    }

    /** Identity and paired devices (the app's `dataDir()`). */
    fun dataDir(context: Context): File = context.filesDir

    /** Download/Nudge, as the app asks for it, before checking it's writable. */
    @Suppress("DEPRECATION") // Still the only way to get the shared Downloads path.
    fun wantedDownloads(): File =
        File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "Nudge")

    /**
     * Where received files go: Download/Nudge, or the app's own folder if
     * that isn't writable (Android 10 without storage permission). Same as
     * the app's `downloadDir()`.
     */
    fun downloads(context: Context): File {
        val wanted = wantedDownloads()
        if (writable(wanted)) return wanted
        return File(context.getExternalFilesDir(null) ?: context.filesDir, "Received")
    }

    private fun writable(dir: File): Boolean = runCatching {
        dir.mkdirs()
        val probe = File(dir, ".nudge-write-test")
        probe.writeText("")
        probe.delete()
        true
    }.getOrDefault(false)

    private var multicastLock: WifiManager.MulticastLock? = null

    /** Android filters multicast on Wi-Fi unless an app holds this; without it, discovery hears nothing. */
    @Synchronized
    fun holdMulticast(context: Context) {
        if (multicastLock?.isHeld == true) return
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicastLock = wifi.createMulticastLock("nudge-discovery").apply {
            setReferenceCounted(false)
            acquire()
        }
    }
}
