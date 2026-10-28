package io.github.darrenn19.nudge

import android.content.Context
import android.media.MediaScannerConnection
import android.os.ParcelFileDescriptor
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File

/**
 * Questions only Android can answer, and things only it can do, for the
 * window: names and folders, the multicast lock, notifications and the
 * background service.
 */
class PlatformChannel(private val context: Context) : MethodChannel.MethodCallHandler {
    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "deviceName" -> result.success(Device.name(context))
            "downloadsDir" -> result.success(Device.wantedDownloads().path)
            "acquireMulticastLock" -> {
                Device.holdMulticast(context)
                result.success(null)
            }
            "instantClipboard" -> result.success(
                mapOf(
                    "overlay" to ClipWatch.canDrawOverlay(context),
                    "logs" to ClipWatch.canReadLogs(context),
                    "running" to ClipWatch.running(),
                ),
            )
            "openOverlaySettings" -> {
                val intent = android.content.Intent(
                    android.provider.Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                    android.net.Uri.parse("package:${context.packageName}"),
                ).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                runCatching { context.startActivity(intent) }
                result.success(null)
            }
            "startInstantClipboard" -> {
                ClipWatch.start(context, again = true)
                result.success(null)
            }
            "latestTileWhileLocked" -> result.success(Latest.whileLocked(context))
            "showScreenOn" -> {
                MirrorConsentActivity.showOn(context, call.argument<String>("device") ?: "")
                result.success(null)
            }
            "setLatestTileWhileLocked" -> {
                Latest.setWhileLocked(context, call.argument<Boolean>("on") == true)
                result.success(null)
            }
            "whenLockedChanged" -> {
                LockState.apply(context)
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
                NudgeService.showProgress(
                    context,
                    call.argument<String>("title") ?: "Nudge",
                    call.argument<String>("text") ?: "",
                    call.argument<Int>("progress") ?: -1,
                    // Dart sends small IDs as Int and big ones as Long.
                    (call.argument<List<Number>>("ids") ?: emptyList()).map { it.toLong() }.toLongArray(),
                )
                result.success(null)
            }
            "transfersIdle" -> {
                NudgeService.idle(context)
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
            "notifyOffer" -> {
                Notifications.offer(
                    context,
                    call.argument<Number>("id")?.toLong() ?: 0,
                    call.argument<String>("title") ?: "",
                    call.argument<String>("text") ?: "",
                )
                result.success(null)
            }
            "cancelOffer" -> {
                call.argument<Number>("id")?.toLong()?.let { Notifications.cancelOffer(context, it) }
                result.success(null)
            }
            "startHotspot" -> Hotspot.start(context) { result.success(it) }
            "stopHotspot" -> {
                Hotspot.stop()
                result.success(null)
            }
            "setClipboard" -> {
                call.argument<String>("text")?.let { ClipSync.set(context, it) }
                result.success(null)
            }
            "clipboardIfNew" -> result.success(ClipSync.ifNew(context))
            "isLocked" -> result.success(LockState.isLocked(context))
            "usbCable" -> result.success(UsbCable.state(context))
            "openTetherSettings" -> {
                UsbCable.openTetherSettings(context)
                result.success(null)
            }
            "openAutoTethering" -> {
                UsbCable.openAutoTethering(context)
                result.success(null)
            }
            "setShareTargets" -> {
                val devices = call.argument<List<Map<String, String>>>("devices").orEmpty()
                    .mapNotNull { d -> d["id"]?.let { id -> id to (d["name"] ?: "Device") } }
                runCatching { ShareTargets.publish(context, devices) }
                result.success(null)
            }
            "holdNetwork" -> {
                NudgeService.holdNetwork(context, call.argument<Boolean>("on") == true)
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

    companion object {
        const val NAME = "nudge/platform"
    }
}
