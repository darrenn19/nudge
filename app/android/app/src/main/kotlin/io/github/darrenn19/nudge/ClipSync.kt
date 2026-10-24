package io.github.darrenn19.nudge

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.PersistableBundle
import android.widget.Toast
import androidx.core.content.FileProvider
import java.io.File

/**
 * Clipboard sync on the phone. Text and images from other devices are put
 * on the clipboard at once, even in the background (Android allows writing
 * any time). Reading is only allowed while a Nudge window has focus: see
 * [ClipWatch] (instant), [ClipActivity] (the notification's button), and
 * opening Nudge.
 */
object ClipSync {
    /** Copies older than this aren't sent when Nudge opens: they're old news. */
    private const val FRESH_MS = 10 * 60 * 1000L

    private var lastText: String? = null

    fun set(context: Context, text: String) {
        val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
        ClipWatch.quietForOwnChange()
        val clip = ClipData.newPlainText("Nudge", text)
        clipboard.setPrimaryClip(clip)
        lastText = text
        remember(context, clipboard.primaryClipDescription)
        // Android 13+ shows its own "copied" confirmation.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            Toast.makeText(context, "Copied from your other device", Toast.LENGTH_SHORT).show()
        }
    }

    /** The clipboard's text, if it changed since last asked and is recent. */
    fun ifNew(context: Context): String? {
        val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return null
        val description = clipboard.primaryClipDescription ?: return null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val stamp = description.timestamp
            val prefs = context.getSharedPreferences("nudge", Context.MODE_PRIVATE)
            if (stamp <= prefs.getLong(STAMP, 0)) return null
            prefs.edit().putLong(STAMP, stamp).apply()
            if (System.currentTimeMillis() - stamp > FRESH_MS) return null
        }
        if (!description.hasMimeType("text/*")) return null
        if (isSensitive(description)) return null
        val text = clipboard.primaryClip?.getItemAt(0)?.coerceToText(context)?.toString()
        if (text.isNullOrEmpty() || text == lastText) return null
        lastText = text
        return text
    }

    /** What's on the clipboard: text, or an image (read with [readImage]). */
    sealed class Item {
        class Text(val text: String) : Item()
        class Image(val uri: Uri, val mime: String) : Item()
    }

    /** The last image Nudge put on the clipboard: not sent back. */
    private var lastImage: Uri? = null

    /** Biggest image sent (as the core allows). */
    private const val MAX_IMAGE = 48L shl 20

    /**
     * What was just copied, for sending: text, or an image. Null if it's
     * private (passwords), neither, or (unless [again]) what was last sent
     * or received. Needs a Nudge window with focus. Marked as sent.
     */
    fun copiedItem(context: Context, again: Boolean = false): Item? {
        val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return null
        val description = clipboard.primaryClipDescription ?: return null
        if (isSensitive(description)) return null
        val item = clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0) ?: return null
        val uri = item.uri
        val imageType = (0 until description.mimeTypeCount)
            .map { description.getMimeType(it) }
            .firstOrNull { it.startsWith("image/") }
        if (uri != null && (imageType != null || context.contentResolver.getType(uri)?.startsWith("image/") == true)) {
            if (uri == lastImage && !again) return null
            val mime = context.contentResolver.getType(uri)?.takeIf { it.startsWith("image/") }
                ?: imageType ?: "image/png"
            lastText = null
            remember(context, description)
            return Item.Image(uri, mime)
        }
        if (!description.hasMimeType("text/*") && item.text == null) return null
        val text = item.coerceToText(context)?.toString()
        if (text.isNullOrEmpty() || (text == lastText && !again)) return null
        lastText = text
        remember(context, description)
        return Item.Text(text)
    }

    /** An image's bytes, or null if unreadable or too big. Off the main thread. */
    fun readImage(context: Context, image: Item.Image): ByteArray? = runCatching {
        context.contentResolver.openInputStream(image.uri)?.use { input ->
            val out = java.io.ByteArrayOutputStream()
            val buf = ByteArray(256 * 1024)
            var total = 0L
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                total += n
                if (total > MAX_IMAGE) return@use null
                out.write(buf, 0, n)
            }
            out.toByteArray()
        }
    }.getOrNull()

    /** An image from another device, saved at [file], onto the clipboard. */
    fun setImage(context: Context, file: File, mime: String) {
        val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
        val uri = runCatching {
            FileProvider.getUriForFile(context, "${context.packageName}.files", file)
        }.getOrNull() ?: return
        ClipWatch.quietForOwnChange()
        val clip = ClipData(ClipDescription("Image from Nudge", arrayOf(mime)), ClipData.Item(uri))
        clipboard.setPrimaryClip(clip)
        lastImage = uri
        lastText = null
        remember(context, clipboard.primaryClipDescription)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            Toast.makeText(context, "Image copied from your other device", Toast.LENGTH_SHORT).show()
        }
    }

    /**
     * Send what [copiedItem] found to every device that shares the
     * clipboard. Returns the names reached. Blocks: off the main thread.
     */
    fun send(context: Context, item: Item): List<String> {
        val json = when (item) {
            is Item.Text -> runCatching { Core.sendClipboard(null, item.text) }.getOrNull()
            is Item.Image -> {
                val data = readImage(context, item) ?: return emptyList()
                runCatching { Core.sendClipboardImage(null, item.mime, data) }.getOrNull()
            }
        } ?: return emptyList()
        val reached = runCatching { org.json.JSONArray(json) }.getOrNull() ?: return emptyList()
        return (0 until reached.length()).map { reached.getString(it) }
    }

    private fun remember(context: Context, description: ClipDescription?) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || description == null) return
        context.getSharedPreferences("nudge", Context.MODE_PRIVATE)
            .edit().putLong(STAMP, description.timestamp).apply()
    }

    /** Password managers mark what they copy as sensitive; never send that. */
    private fun isSensitive(description: ClipDescription): Boolean {
        val extras: PersistableBundle = description.extras ?: return false
        return extras.getBoolean("android.content.extra.IS_SENSITIVE", false)
    }

    private const val STAMP = "clipStamp"
}
