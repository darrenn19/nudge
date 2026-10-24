package io.github.darrenn19.nudge

import android.Manifest
import android.content.ContentUris
import android.content.Context
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.MediaStore
import android.widget.Toast

/**
 * The newest photo or video on the phone, sent to your device (the
 * "Send latest" tile): a picture just taken, a screenshot, or an image or
 * video just downloaded. Android lets apps see other apps' photos and
 * videos, not their other files, so a downloaded PDF isn't included.
 * Files Nudge itself received aren't either.
 */
object Latest {
    /** Reading photos and videos. */
    val permissions: Array<String> =
        if (Build.VERSION.SDK_INT >= 33) {
            arrayOf(Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VIDEO)
        } else {
            arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE)
        }

    fun allowed(context: Context) = permissions.any {
        context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
    }

    /** Whether the tile may send while the phone is locked (off unless turned on). */
    fun whileLocked(context: Context) =
        context.getSharedPreferences("nudge", Context.MODE_PRIVATE).getBoolean(WHILE_LOCKED, false)

    fun setWhileLocked(context: Context, on: Boolean) =
        context.getSharedPreferences("nudge", Context.MODE_PRIVATE).edit().putBoolean(WHILE_LOCKED, on).apply()

    enum class Result { SENT, CHOOSE, NONE }

    /**
     * Send the newest photo or video to the default, obvious or current
     * device ([Targets.forLatest]): [Result.CHOOSE] when there's still a
     * choice to make (the caller opens the window), or
     * [Result.NONE] with nothing to send. Quick (two media queries); the
     * sending happens in the core.
     */
    fun send(context: Context): Result {
        val file = find(context) ?: return Result.NONE
        val target = Targets.forLatest(context) ?: return Result.CHOOSE
        val pfd = runCatching { context.contentResolver.openFileDescriptor(file.uri, "r") }.getOrNull()
            ?: return Result.NONE
        val fd = pfd.detachFd()
        return if (Core.sendOpenFiles(target.first, intArrayOf(fd), arrayOf(file.name))) {
            say(context, "Sending ${file.name} to ${target.second}")
            Result.SENT
        } else {
            runCatching { android.os.ParcelFileDescriptor.adoptFd(fd).close() }
            Result.CHOOSE
        }
    }

    class File(val uri: Uri, val name: String, val added: Long)

    /** The newest photo or video, or null (none, or no permission). */
    fun find(context: Context): File? = listOfNotNull(
        newest(context, MediaStore.Images.Media.EXTERNAL_CONTENT_URI),
        newest(context, MediaStore.Video.Media.EXTERNAL_CONTENT_URI),
    ).maxByOrNull { it.added }

    private fun newest(context: Context, collection: Uri): File? {
        val columns = arrayOf(
            MediaStore.MediaColumns._ID,
            MediaStore.MediaColumns.DISPLAY_NAME,
            MediaStore.MediaColumns.DATE_ADDED,
        )
        // Not what Nudge received: sending it straight back isn't the point.
        val (where, args) = if (Build.VERSION.SDK_INT >= 29) {
            "${MediaStore.MediaColumns.RELATIVE_PATH} NOT LIKE ?" to arrayOf("Download/Nudge%")
        } else {
            null to null
        }
        return runCatching {
            context.contentResolver.query(
                collection,
                columns,
                where,
                args,
                "${MediaStore.MediaColumns.DATE_ADDED} DESC",
            )?.use { c ->
                if (!c.moveToFirst()) return@use null
                File(ContentUris.withAppendedId(collection, c.getLong(0)), c.getString(1) ?: "file", c.getLong(2))
            }
        }.getOrNull()
    }

    fun say(context: Context, text: String) = Handler(Looper.getMainLooper()).post {
        Toast.makeText(context.applicationContext, text, Toast.LENGTH_SHORT).show()
    }

    private const val WHILE_LOCKED = "screenshotTileWhileLocked"
}
