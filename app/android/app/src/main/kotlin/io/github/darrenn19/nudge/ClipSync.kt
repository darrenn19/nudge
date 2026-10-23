package io.github.darrenn19.nudge

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.PersistableBundle
import android.widget.Toast

/**
 * Clipboard sync on the phone. Text from other devices is put on the
 * clipboard at once, even in the background (Android allows writing any
 * time). Reading is only allowed while Nudge is on screen, so text copied
 * here goes out when Nudge opens, or when shared to Nudge.
 */
object ClipSync {
    /** Copies older than this aren't sent when Nudge opens: they're old news. */
    private const val FRESH_MS = 10 * 60 * 1000L

    private var lastText: String? = null

    fun set(context: Context, text: String) {
        val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
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
