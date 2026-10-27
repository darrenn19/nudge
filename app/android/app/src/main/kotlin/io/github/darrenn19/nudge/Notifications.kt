package io.github.darrenn19.nudge

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.webkit.MimeTypeMap
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.FileProvider
import java.io.File
import java.util.concurrent.atomic.AtomicInteger

/** Nudge's notifications: "ready to receive", transfer progress, and "received". */
object Notifications {
    private const val READY = "ready"
    private const val OFFERS = "offers"
    private const val TRANSFERS = "transfers"
    private const val RECEIVED = "received"
    private const val RECEIVED_GROUP = "io.github.darrenn19.nudge.RECEIVED"
    /** The service's ongoing notification: "ready" or progress. */
    const val SERVICE_ID = 1
    /** "Instant clipboard is paused" (see [clipboardPaused]). */
    private const val CLIP_PAUSED_ID = 2
    private val nextId = AtomicInteger(1000)

    fun createChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannels(
            listOf(
                // Minimised: no status bar icon or sound, just a line at the
                // bottom of the shade. Users can turn it off here.
                NotificationChannel(READY, "Ready to receive", NotificationManager.IMPORTANCE_MIN)
                    .apply { setShowBadge(false) },
                // Someone wants to send you files: worth interrupting for.
                NotificationChannel(OFFERS, "Incoming files", NotificationManager.IMPORTANCE_HIGH),
                NotificationChannel(TRANSFERS, "Transfers in progress", NotificationManager.IMPORTANCE_LOW)
                    .apply { setShowBadge(false) },
                NotificationChannel(RECEIVED, "Received files", NotificationManager.IMPORTANCE_DEFAULT),
            ),
        )
    }

    /** Shown while Nudge waits in the background for files. */
    fun ready(context: Context): Notification =
        NotificationCompat.Builder(context, READY)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Ready to receive")
            .setContentText("Your paired devices can send files here")
            .setOngoing(true)
            .setSilent(true)
            .setShowWhen(false)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setContentIntent(openApp(context))
            // Android won't let Nudge read the clipboard in the background:
            // one tap here sends it (see ClipActivity).
            .addAction(0, "Send clipboard", sendClipboard(context))
            .build()

    private fun sendClipboard(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        3,
        Intent(context, ClipActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        PendingIntent.FLAG_IMMUTABLE,
    )

    /**
     * The ongoing notification shown while files are moving. [progress] is
     * 0–100, or -1 if unknown. Its Cancel stops [ids].
     */
    fun progress(context: Context, title: String, text: String, progress: Int, ids: LongArray): Notification =
        NotificationCompat.Builder(context, TRANSFERS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(text)
            .setProgress(100, progress.coerceIn(0, 100), progress < 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setContentIntent(openApp(context))
            .addAction(0, "Cancel", cancel(context, ids))
            .build()

    private fun cancel(context: Context, ids: LongArray): PendingIntent = PendingIntent.getBroadcast(
        context,
        CANCEL_REQUEST,
        Intent(context, CancelReceiver::class.java).putExtra(CancelReceiver.EXTRA_IDS, ids),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private const val CANCEL_REQUEST = 1900

    /** "Received photo.jpg from Laptop", with an action that opens the file. */
    /**
     * [title] names what came ("photo.jpg", or for several files at once
     * "Trip" or "37 files"); [file] is the file, or the folder they went in.
     */
    fun received(context: Context, file: File, from: String, title: String = file.name) {
        // Viewer apps open files, not folders: a folder opens Nudge.
        val open = if (file.isDirectory) null else openFile(context, file)
        val notification = NotificationCompat.Builder(context, RECEIVED)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Received $title")
            .setContentText(if (from.isBlank()) "Saved to ${file.parentFile?.name ?: "Downloads"}" else "From $from")
            .setAutoCancel(true)
            .setGroup(RECEIVED_GROUP)
            // On the lock screen: no file or device names until unlocked.
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(lockScreen(context, RECEIVED, "File received"))
            .setContentIntent(open ?: openApp(context))
            .apply { open?.let { addAction(0, "Open", it) } }
            .build()
        notify(context, nextId.getAndIncrement(), notification)
    }

    /**
     * "Darren's PC wants to send you 3 files", with Accept and Decline; with
     * [pairing], "Darren's PC wants to pair" instead.
     */
    fun offer(context: Context, id: Long, title: String, text: String, pairing: Boolean = false) {
        fun answer(accept: Boolean) = PendingIntent.getBroadcast(
            context,
            (id * 2 + if (accept) 1 else 0).toInt(),
            Intent(context, OfferActionReceiver::class.java)
                .putExtra(OfferActionReceiver.EXTRA_ID, id)
                .putExtra(OfferActionReceiver.EXTRA_ACCEPT, accept)
                .putExtra(OfferActionReceiver.EXTRA_PAIRING, pairing),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = NotificationCompat.Builder(context, OFFERS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(text)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            // On the lock screen: no names, and answering needs an unlock.
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(lockScreen(context, OFFERS, "A device wants to send you files"))
            .setAutoCancel(true)
            // The question lapses after a minute anyway.
            .setTimeoutAfter(60_000)
            .setContentIntent(openApp(context))
            .addAction(0, "Decline", answer(false))
            .addAction(0, if (pairing) "Pair" else "Accept", answer(true))
            .build()
        notify(context, offerId(id), notification)
    }

    fun cancelOffer(context: Context, id: Long) = cancel(context, offerId(id))

    /** What a private notification shows on the lock screen. */
    private fun lockScreen(context: Context, channel: String, title: String): Notification =
        NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .build()

    private fun offerId(id: Long) = 2000 + (id % 100_000).toInt()

    fun notify(context: Context, id: Int, notification: Notification) {
        // Without the notification permission (Android 13+), this does nothing.
        runCatching { NotificationManagerCompat.from(context).notify(id, notification) }
    }

    fun cancel(context: Context, id: Int) = NotificationManagerCompat.from(context).cancel(id)

    /**
     * Instant clipboard stopped (Nudge restarted: a reboot, an update) and
     * Android only lets it start again while Nudge is on screen. Quiet.
     */
    fun clipboardPaused(context: Context) {
        val notification = NotificationCompat.Builder(context, TRANSFERS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Instant clipboard is paused")
            .setContentText("Tap to turn it back on")
            .setSilent(true)
            .setAutoCancel(true)
            .setContentIntent(openApp(context))
            .build()
        runCatching { NotificationManagerCompat.from(context).notify(CLIP_PAUSED_ID, notification) }
    }

    fun clipboardResumed(context: Context) = cancel(context, CLIP_PAUSED_ID)

    private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private fun openFile(context: Context, file: File): PendingIntent? {
        val uri = runCatching {
            FileProvider.getUriForFile(context, "${context.packageName}.files", file)
        }.getOrNull() ?: return null
        val type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase()) ?: "*/*"
        val view = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, type)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        return PendingIntent.getActivity(
            context,
            file.path.hashCode(),
            view,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }
}
