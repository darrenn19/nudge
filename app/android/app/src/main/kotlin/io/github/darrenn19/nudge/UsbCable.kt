package io.github.darrenn19.nudge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.provider.Settings
import androidx.core.content.ContextCompat
import java.io.File

/**
 * A cable to a computer, and whether files can go over it.
 *
 * Files go over a cable when the phone shares a network on it (USB
 * tethering): the computer then reaches the phone at an address on that
 * network, and the core prefers it (see core/src/link.rs). The kind of cable
 * doesn't matter (USB-C, micro-USB, USB-A to anything), as long as it
 * carries data. An app can't turn tethering on, so the window offers to
 * open the screen where the person can.
 */
object UsbCable {
    /** Android's (hidden but long-standing) sticky broadcast for the USB port. */
    private const val USB_STATE = "android.hardware.usb.action.USB_STATE"

    /**
     * Set once the person has been shown how to use the cable: after that,
     * neither this phone nor its computers ask again. Android turns USB
     * tethering off whenever the cable comes out, and no app may turn it on,
     * so the one ask is to make it come back by itself (Developer options →
     * Default USB configuration → USB tethering).
     */
    private fun askedFile(context: Context) = File(Device.dataDir(context), "cable_asked.txt")

    fun asked(context: Context): Boolean = askedFile(context).exists()

    private fun markAsked(context: Context) {
        runCatching { askedFile(context).writeText("Shown how to send over a cable.\n") }
        tell(context)
    }

    /** Developer options is on (it's hidden until Build number is tapped 7 times). */
    fun developerOptions(context: Context): Boolean = Settings.Global.getInt(
        context.contentResolver,
        Settings.Global.DEVELOPMENT_SETTINGS_ENABLED,
        0,
    ) == 1

    /**
     * `data`: a computer at the other end, talking USB (not just power).
     * `tethering`: the phone shares a network over it (RNDIS, or NCM on
     * newer phones). `chargeOnly`: power from a computer but no data, which
     * means a charge-only cable (or a computer that's asleep).
     */
    fun state(context: Context): Map<String, Boolean> {
        val usb = context.registerReceiver(null, IntentFilter(USB_STATE))
        val battery = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val plugged = battery?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0
        val connected = usb?.getBooleanExtra("connected", false) == true
        val configured = usb?.getBooleanExtra("configured", false) == true
        val tethering = usb?.getBooleanExtra("rndis", false) == true ||
            usb?.getBooleanExtra("ncm", false) == true
        return mapOf(
            "data" to (connected && configured),
            "tethering" to (connected && tethering),
            "chargeOnly" to (!connected && plugged == BatteryManager.BATTERY_PLUGGED_USB),
            "asked" to asked(context),
            "developerOptions" to developerOptions(context),
        )
    }

    /** Tell the core whether to say "turn on USB tethering" (see [tellCore]). */
    private fun tell(context: Context) {
        val now = state(context)
        Core.setCableUnused(now["data"] == true && now["tethering"] != true && now["asked"] != true)
    }

    /**
     * Keep the core told whether the cable could carry files but doesn't
     * (tethering off), for as long as the process lives: paired computers
     * then say what to switch on.
     */
    fun tellCore(context: Context) {
        val app = context.applicationContext
        tell(app)
        watch(app) { tell(app) }
    }

    /** Calls [onChange] whenever the cable or its use changes. */
    fun watch(context: Context, onChange: () -> Unit): BroadcastReceiver {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) = onChange()
        }
        val filter = IntentFilter().apply {
            addAction(USB_STATE)
            addAction(Intent.ACTION_POWER_CONNECTED)
            addAction(Intent.ACTION_POWER_DISCONNECTED)
        }
        // System broadcasts only; nothing else needs to reach it.
        ContextCompat.registerReceiver(context, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
        return receiver
    }

    /**
     * Where to make USB tethering come on by itself whenever the phone is
     * plugged into a computer: Developer options, at Default USB
     * configuration. With Developer options still hidden, About phone,
     * where tapping Build number shows it.
     */
    fun openAutoTethering(context: Context) {
        markAsked(context)
        val intent = if (developerOptions(context)) {
            Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)
                // Settings scrolls to (and highlights) the item.
                .putExtra(":settings:fragment_args_key", "default_usb_configuration")
        } else {
            Intent(Settings.ACTION_DEVICE_INFO_SETTINGS)
        }
        val opened = runCatching {
            context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }.isSuccess
        if (!opened) openTetherSettings(context)
    }

    /** The screen with the USB tethering switch, or as near as there is. */
    fun openTetherSettings(context: Context) {
        markAsked(context)
        val tries = listOf(
            // Stock Android and Samsung ("Mobile Hotspot and Tethering").
            Intent("android.settings.TETHER_SETTINGS"),
            Intent().setClassName("com.android.settings", "com.android.settings.TetherSettings"),
            Intent(Settings.ACTION_WIRELESS_SETTINGS),
            Intent(Settings.ACTION_SETTINGS),
        )
        for (intent in tries) {
            val opened = runCatching {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }.isSuccess
            if (opened) return
        }
    }
}
