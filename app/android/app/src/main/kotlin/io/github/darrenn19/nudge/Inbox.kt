package io.github.darrenn19.nudge

/**
 * Files waiting for the window to choose where they go: shared when no
 * device was obvious, picked from the tile, or meant for someone without
 * Nudge. Each is a map for Dart (see [SharedFiles]): `name`, `size`, `fd` or
 * `path`, and maybe `byQr` or `to`. The window takes them with `takeShared`.
 */
object Inbox {
    private val waiting = mutableListOf<Map<String, Any>>()

    @Synchronized
    fun add(files: List<Map<String, Any>>) {
        waiting.addAll(files)
    }

    @Synchronized
    fun take(): List<Map<String, Any>> = waiting.toList().also { waiting.clear() }
}
