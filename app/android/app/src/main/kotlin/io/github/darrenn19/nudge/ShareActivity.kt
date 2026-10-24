package io.github.darrenn19.nudge

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.widget.Toast
import org.json.JSONArray
import java.io.File

/**
 * Where shares land: the share sheet ("Nudge", a device in its top row,
 * "Share by QR code") and "Send with Nudge" on selected text. Shows nothing.
 *
 * When it's clear where things go (a device picked in the share sheet, or
 * only one device around) it hands them to the core and finishes, and the
 * notification shows progress. Only when there's a choice to make does it
 * open the app's window.
 */
class ShareActivity : Activity() {
    private val main = Handler(Looper.getMainLooper())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val intent = intent
        // "Share by QR code": for someone without Nudge.
        val byQr = intent.component?.className?.endsWith(SHARE_BY_QR) == true
        // A device picked in the share sheet's top row (Direct Share).
        val to = ShareTargets.deviceOf(intent)
        // Connect while the files are still being opened.
        if (to != null) runCatching { Core.warm(to) }
        val text = when (intent.action) {
            Intent.ACTION_PROCESS_TEXT -> intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
            Intent.ACTION_SEND -> intent.getStringExtra(Intent.EXTRA_TEXT).takeIf { intent.stream() == null }
            else -> null
        }
        val uris = when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(intent.stream())
            Intent.ACTION_SEND_MULTIPLE -> intent.streams()
            else -> emptyList()
        }
        // Opening can mean copying (see SharedFiles), so it happens off the
        // main thread; the activity stays (invisibly) until the files are
        // open, since the permission to read them is its.
        Thread {
            when {
                !text.isNullOrBlank() && !byQr -> copyAcross(text, to)
                !text.isNullOrBlank() -> toWindow(listOf(SharedFiles.text(this, text)), byQr = true)
                uris.isNotEmpty() -> send(uris.mapNotNull { SharedFiles.open(this, it) }, byQr, to)
            }
            main.post { finish() }
        }.start()
    }

    /** A link or a note: onto [to]'s clipboard, or every syncing device's. */
    private fun copyAcross(text: String, to: String?) {
        val reached = runCatching { JSONArray(Core.sendClipboard(to, text)) }.getOrNull()
        val names = (0 until (reached?.length() ?: 0)).map { reached!!.getString(it) }
        say(if (names.isEmpty()) "Couldn't reach a device to copy to" else "Copied to ${names.joinToString(", ")}")
    }

    private fun send(files: List<Map<String, Any>>, byQr: Boolean, to: String?) {
        if (files.isEmpty()) return
        val device = when {
            byQr -> null
            to != null -> to to Targets.name(to)
            else -> Targets.onlyChoice()
        }
        if (device == null) {
            toWindow(files, byQr)
            return
        }
        val fds = files.map { f ->
            (f["fd"] as? Int) ?: runCatching {
                ParcelFileDescriptor.open(File(f["path"] as String), ParcelFileDescriptor.MODE_READ_ONLY).detachFd()
            }.getOrDefault(-1)
        }
        val names = files.map { it["name"] as String }
        if (Core.sendOpenFiles(device.first, fds.toIntArray(), names.toTypedArray())) {
            say("Sending ${if (names.size == 1) names.single() else "${names.size} files"} to ${device.second}")
        } else {
            // Opened here only for sending: the window opens its own.
            files.zip(fds).filter { (f, fd) -> f["fd"] == null && fd >= 0 }
                .forEach { (_, fd) -> runCatching { ParcelFileDescriptor.adoptFd(fd).close() } }
            toWindow(files, byQr = false)
        }
    }

    /** Let the window decide: open it with the files waiting. */
    private fun toWindow(files: List<Map<String, Any>>, byQr: Boolean) {
        if (files.isEmpty()) return
        Inbox.add(if (byQr) files.map { it + ("byQr" to true) } else files)
        startActivity(
            Intent(this, MainActivity::class.java)
                .setAction(MainActivity.ACTION_SHARED)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }

    private fun say(text: String) = main.post {
        Toast.makeText(applicationContext, text, Toast.LENGTH_SHORT).show()
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
        /** The share sheet entry declared as an alias of this activity. */
        private const val SHARE_BY_QR = ".ShareByQr"
    }
}
