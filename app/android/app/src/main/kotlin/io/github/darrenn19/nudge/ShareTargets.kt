package io.github.darrenn19.nudge

import android.content.Context
import android.content.Intent
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat

/**
 * Direct Share: each paired device in the share sheet's top row, so sharing
 * to "My Laptop" sends straight there. Sharing to one opens [MainActivity]
 * with the shortcut's ID; [deviceOf] turns that back into the device.
 */
object ShareTargets {
    private const val CATEGORY = "io.github.darrenn19.nudge.category.SEND_TO_DEVICE"
    private const val PREFIX = "device:"

    /** Replace the published devices with [devices] (ID to name), in order. */
    fun publish(context: Context, devices: List<Pair<String, String>>) {
        val max = ShortcutManagerCompat.getMaxShortcutCountPerActivity(context).coerceAtLeast(1)
        val shortcuts = devices.take(max).mapIndexed { rank, (id, name) ->
            ShortcutInfoCompat.Builder(context, PREFIX + id)
                .setShortLabel(name)
                .setIcon(IconCompat.createWithResource(context, R.mipmap.ic_launcher))
                // Opened from a launcher long-press: just the app.
                .setIntent(Intent(context, MainActivity::class.java).setAction(Intent.ACTION_MAIN))
                .setCategories(setOf(CATEGORY))
                .setLongLived(true)
                .setRank(rank)
                .build()
        }
        val kept = shortcuts.map { it.id }.toSet()
        val gone = ShortcutManagerCompat.getShortcuts(
            context,
            ShortcutManagerCompat.FLAG_MATCH_DYNAMIC or ShortcutManagerCompat.FLAG_MATCH_CACHED,
        ).map { it.id }.filter { it.startsWith(PREFIX) && it !in kept }
        // Forgotten devices leave the share sheet too.
        if (gone.isNotEmpty()) ShortcutManagerCompat.removeLongLivedShortcuts(context, gone)
        ShortcutManagerCompat.setDynamicShortcuts(context, shortcuts)
    }

    /** The device a share was aimed at, if it came through one of these. */
    fun deviceOf(intent: Intent): String? =
        intent.getStringExtra(ShortcutManagerCompat.EXTRA_SHORTCUT_ID)
            ?.takeIf { it.startsWith(PREFIX) }
            ?.removePrefix(PREFIX)
}
