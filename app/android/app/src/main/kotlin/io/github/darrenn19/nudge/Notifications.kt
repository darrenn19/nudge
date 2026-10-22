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
            .build()

    /** The ongoing notification shown while files are moving. [progress] is 0–100, or -1 if unknown. */
    fun progress(context: Context, title: String, text: String, progress: Int): Notification =
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
            .build()

    /** "Received photo.jpg from Laptop", with an action that opens the file. */
    fun received(context: Context, file: File, from: String) {
        val notification = NotificationCompat.Builder(context, RECEIVED)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Received ${file.name}")
            .setContentText(if (from.isBlank()) "Saved to ${file.parentFile?.name ?: "Downloads"}" else "From $from")
            .setAutoCancel(true)
            .setGroup(RECEIVED_GROUP)
            .setContentIntent(openFile(context, file) ?: openApp(context))
            .apply { openFile(context, file)?.let { addAction(0, "Open", it) } }
            .build()
        notify(context, nextId.getAndIncrement(), notification)
    }

    /** "Darren's PC wants to send you 3 files", with Accept and Decline. */
    fun offer(context: Context, id: Long, title: String, text: String) {
        fun answer(accept: Boolean) = PendingIntent.getBroadcast(
            context,
            (id * 2 + if (accept) 1 else 0).toInt(),
            Intent(context, OfferActionReceiver::class.java)
                .putExtra(OfferActionReceiver.EXTRA_ID, id)
                .putExtra(OfferActionReceiver.EXTRA_ACCEPT, accept),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = NotificationCompat.Builder(context, OFFERS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(text)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            // The question lapses after a minute anyway.
            .setTimeoutAfter(60_000)
            .setContentIntent(openApp(context))
            .addAction(0, "Decline", answer(false))
            .addAction(0, "Accept", answer(true))
            .build()
        notify(context, offerId(id), notification)
    }

    fun cancelOffer(context: Context, id: Long) = cancel(context, offerId(id))

    private fun offerId(id: Long) = 2000 + (id % 100_000).toInt()

    fun notify(context: Context, id: Int, notification: Notification) {
        // Without the notification permission (Android 13+), this does nothing.
        runCatching { NotificationManagerCompat.from(context).notify(id, notification) }
    }

    fun cancel(context: Context, id: Int) = NotificationManagerCompat.from(context).cancel(id)

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
