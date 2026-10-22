package io.github.darrenn19.nudge

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.system.Os
import android.system.OsConstants
import java.io.File

/**
 * Turns shared or picked content URIs into things the core can send.
 *
 * Regular files become open descriptors: the core reads them directly, so a
 * 4 GB video starts sending at once instead of being copied first. Anything
 * else (a pipe some apps stream generated content through) is copied to the
 * cache, since sending needs a known size and the ability to resume.
 *
 * Each result is a map for Dart: `name`, `size`, and either `fd` or `path`.
 */
object SharedFiles {
    fun open(context: Context, uri: Uri): Map<String, Any>? = runCatching {
        if (uri.scheme == "file") {
            val file = File(requireNotNull(uri.path))
            return mapOf("path" to file.path, "name" to file.name, "size" to file.length())
        }
        val name = displayName(context, uri)
        val pfd = context.contentResolver.openFileDescriptor(uri, "r") ?: return null
        val stat = Os.fstat(pfd.fileDescriptor)
        if (OsConstants.S_ISREG(stat.st_mode)) {
            return mapOf("fd" to pfd.detachFd(), "name" to name, "size" to stat.st_size)
        }
        pfd.close()
        val copy = outgoingFile(context, name)
        context.contentResolver.openInputStream(uri)?.use { input ->
            copy.outputStream().use { input.copyTo(it, 1 shl 20) }
        } ?: return null
        mapOf("path" to copy.path, "name" to name, "size" to copy.length())
    }.getOrNull()

    /** Text shared on its own (a link, a note) goes over as a .txt file. */
    fun text(context: Context, text: String): Map<String, Any> {
        val file = outgoingFile(context, "Shared text.txt")
        file.writeText(text)
        return mapOf("path" to file.path, "name" to file.name, "size" to file.length())
    }

    /** Copies made by earlier sessions. Called at startup. */
    fun clearCopies(context: Context) {
        File(context.cacheDir, "outgoing").deleteRecursively()
    }

    private fun outgoingFile(context: Context, name: String): File {
        val dir = File(context.cacheDir, "outgoing/${System.nanoTime()}").apply { mkdirs() }
        return File(dir, name)
    }

    private fun displayName(context: Context, uri: Uri): String {
        val queried = runCatching {
            context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
        }.getOrNull()
        val name = queried ?: uri.lastPathSegment?.substringAfterLast('/')
        return name?.takeIf { it.isNotBlank() } ?: "file"
    }
}
