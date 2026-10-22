package io.github.darrenn19.nudge

import android.content.Context
import android.media.MediaScannerConnection
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.provider.Settings
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File

/**
 * Questions only Android can answer, and things only it can do, that don't
 * need a window: names and folders, the multicast lock, notifications and
 * the foreground service. Lives as long as the process.
 */
class PlatformChannel(private val context: Context) : MethodChannel.MethodCallHandler {
    private var multicastLock: WifiManager.MulticastLock? = null

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "deviceName" -> result.success(deviceName())
            "downloadsDir" -> result.success(downloadsDir())
            "acquireMulticastLock" -> {
                acquireMulticastLock()
                result.success(null)
            }
            "scanFile" -> {
                // Makes a received file show up in Files and Gallery right away.
                call.argument<String>("path")?.let {
                    MediaScannerConnection.scanFile(context, arrayOf(it), null, null)
                }
                result.success(null)
            }
            "transfersActive" -> {
                TransferService.update(
                    context,
                    call.argument<String>("title") ?: "Nudge",
                    call.argument<String>("text") ?: "",
                    call.argument<Int>("progress") ?: -1,
                )
                result.success(null)
            }
            "transfersIdle" -> {
                TransferService.stop(context)
                result.success(null)
            }
            "notifyReceived" -> {
                val path = call.argument<String>("path")
                if (path != null) {
                    MediaScannerConnection.scanFile(context, arrayOf(path), null, null)
                    Notifications.received(context, File(path), call.argument<String>("from") ?: "")
                }
                result.success(null)
            }
            "closeFiles" -> {
                // Shared files the user decided not to send.
                call.argument<List<Int>>("fds")?.forEach { fd ->
                    runCatching { ParcelFileDescriptor.adoptFd(fd).close() }
                }
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    /** The name the user gave the phone (Settings → About phone), else the model. */
    private fun deviceName(): String {
        val named = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
        if (!named.isNullOrBlank()) return named
        val model = Build.MODEL ?: "Android phone"
        val maker = Build.MANUFACTURER ?: ""
        return if (model.startsWith(maker, ignoreCase = true)) model else "$maker $model".trim()
    }

    @Suppress("DEPRECATION") // Still the only way to get the shared Downloads path.
    private fun downloadsDir(): String =
        File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "Nudge").path

    private fun acquireMulticastLock() {
        if (multicastLock?.isHeld == true) return
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicastLock = wifi.createMulticastLock("nudge-discovery").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    companion object {
        const val NAME = "nudge/platform"
    }
}
