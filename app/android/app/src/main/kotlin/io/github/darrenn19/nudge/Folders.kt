package io.github.darrenn19.nudge

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.system.Os
import android.system.OsConstants

/**
 * A folder picked with the system folder picker (a "tree" URI), as files
 * to send: each opened as a descriptor and named by its path in the folder
 * ("Trip/day 2/plan.jpg"), so it arrives as a folder. The core packs the
 * small ones together. Empty folders are listed too, named with a
 * trailing "/" and no descriptor.
 */
object Folders {
    /** At most this many files from one folder (each holds a descriptor). */
    private const val MAX_FILES = 10_000

    /** Maps for Dart, like [SharedFiles]: `fd`, `name`, `size`. */
    fun open(context: Context, tree: Uri): List<Map<String, Any>> {
        val rootId = DocumentsContract.getTreeDocumentId(tree)
        val rootName = name(context, DocumentsContract.buildDocumentUriUsingTree(tree, rootId)) ?: "Folder"
        val out = mutableListOf<Map<String, Any>>()
        walk(context, tree, rootId, rootName, 0, out)
        return out
    }

    private fun walk(
        context: Context,
        tree: Uri,
        id: String,
        path: String,
        depth: Int,
        out: MutableList<Map<String, Any>>,
    ) {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, id)
        val columns = arrayOf(Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE)
        val entries = runCatching {
            context.contentResolver.query(children, columns, null, null, null)?.use { c ->
                buildList {
                    while (c.moveToNext()) add(Triple(c.getString(0), c.getString(1) ?: "file", c.getString(2)))
                }
            }
        }.getOrNull().orEmpty().sortedBy { it.second }
        if (entries.isEmpty()) {
            out.add(mapOf("name" to "$path/", "size" to 0L))
            return
        }
        for ((childId, name, mime) in entries) {
            if (out.size >= MAX_FILES) return
            val childPath = "$path/$name"
            if (mime == Document.MIME_TYPE_DIR) {
                if (depth < 63) walk(context, tree, childId, childPath, depth + 1, out)
                continue
            }
            val uri = DocumentsContract.buildDocumentUriUsingTree(tree, childId)
            val pfd = runCatching { context.contentResolver.openFileDescriptor(uri, "r") }.getOrNull() ?: continue
            val stat = runCatching { Os.fstat(pfd.fileDescriptor) }.getOrNull()
            if (stat == null || !OsConstants.S_ISREG(stat.st_mode)) {
                pfd.close() // not a plain file (a stream some app makes up): skipped
                continue
            }
            out.add(mapOf("fd" to pfd.detachFd(), "name" to childPath, "size" to stat.st_size))
        }
    }

    private fun name(context: Context, uri: Uri): String? = runCatching {
        context.contentResolver.query(uri, arrayOf(Document.COLUMN_DISPLAY_NAME), null, null, null)
            ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
    }.getOrNull()
}
