package io.github.darrenn19.nudge

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.SoftApConfiguration
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper

/**
 * The direct link for big transfers: a local-only hotspot, a private
 * network between this phone and the PC with no internet sharing. The PC
 * joins it, so data crosses the air once instead of twice (via the router).
 *
 * Needs Android 8+, and "nearby devices" permission (Android 13+) or
 * location permission with location on (older). Without them it just
 * doesn't start, and the transfer stays on the usual Wi-Fi.
 */
object Hotspot {
    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null

    /** Whether starting one can work at all. */
    fun allowed(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return false
        val permission = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            Manifest.permission.NEARBY_WIFI_DEVICES
        } else {
            Manifest.permission.ACCESS_FINE_LOCATION
        }
        return context.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED
    }

    /** Start (or reuse) the hotspot; [done] gets how to join it, or null. */
    fun start(context: Context, done: (Map<String, Any>?) -> Unit) {
        reservation?.let {
            done(details(it))
            return
        }
        if (!allowed(context)) {
            done(null)
            return
        }
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        var answered = false
        fun answer(result: Map<String, Any>?) {
            if (answered) return
            answered = true
            done(result)
        }
        try {
            wifi.startLocalOnlyHotspot(
                object : WifiManager.LocalOnlyHotspotCallback() {
                    override fun onStarted(started: WifiManager.LocalOnlyHotspotReservation) {
                        reservation = started
                        answer(details(started))
                    }

                    override fun onFailed(reason: Int) = answer(null)

                    override fun onStopped() {
                        reservation = null
                    }
                },
                Handler(Looper.getMainLooper()),
            )
        } catch (e: Exception) {
            // Not allowed right now (permission, location off, tethering on).
            answer(null)
        }
    }

    fun stop() {
        reservation?.close()
        reservation = null
    }

    private fun details(r: WifiManager.LocalOnlyHotspotReservation): Map<String, Any>? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val config = r.softApConfiguration
            val ssid = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                config.wifiSsid?.toString()?.removeSurrounding("\"")
            } else {
                @Suppress("DEPRECATION")
                config.ssid
            } ?: return null
            return mapOf(
                "ssid" to ssid,
                "passphrase" to (config.passphrase ?: ""),
                "wpa3" to (config.securityType == SoftApConfiguration.SECURITY_TYPE_WPA3_SAE),
            )
        }
        @Suppress("DEPRECATION")
        val config = r.wifiConfiguration ?: return null
        @Suppress("DEPRECATION")
        return mapOf(
            "ssid" to config.SSID.removeSurrounding("\""),
            "passphrase" to (config.preSharedKey ?: "").removeSurrounding("\""),
            "wpa3" to false,
        )
    }
}
