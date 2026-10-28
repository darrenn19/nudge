package io.github.darrenn19.nudge

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.SoftApConfiguration
import android.net.wifi.WifiManager
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pGroup
import android.net.wifi.p2p.WifiP2pManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.util.SparseIntArray
import java.security.SecureRandom

/**
 * The direct link for big transfers: a private network between this phone
 * and the PC, with no internet sharing. The PC joins it, so data crosses the
 * air once instead of twice (via the router).
 *
 * Two ways to host one, tried in order:
 * 1. **A Wi-Fi Direct group** (Android 10+) that this phone owns, asked for
 *    on 5 GHz. The PC joins it like any Wi-Fi network: a group owner lets in
 *    ordinary Wi-Fi clients by name and passphrase. Phones keep these on
 *    5 GHz while on Wi-Fi themselves, where a local-only hotspot drops to
 *    2.4 GHz (about 10 MB/s, no faster than the router).
 * 2. **A local-only hotspot**, if the phone can't run a Wi-Fi Direct group
 *    (busy with another one, or not supported).
 *
 * Both need "nearby devices" permission (Android 13+) or location permission
 * with location on (older). Without them nothing starts, and the transfer
 * stays on the usual Wi-Fi.
 */
object Hotspot {
    private const val TAG = "Nudge"

    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null
    private var direct: Pair<WifiP2pManager, WifiP2pManager.Channel>? = null
    private var details: Map<String, Any>? = null

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

    /**
     * Start (or reuse) a direct link; [done] gets how to join it ("ssid",
     * "passphrase", "wpa3", and "mhz": its frequency, 0 if unknown), or null.
     * With [directOnly] (screen mirroring) only a Wi-Fi Direct group will do:
     * a local-only hotspot is often on 2.4 GHz or a second channel the radio
     * hops to, which is worse than the router for live video.
     */
    fun start(context: Context, directOnly: Boolean = false, done: (Map<String, Any>?) -> Unit) {
        details?.let {
            if (!directOnly || direct != null) {
                done(it)
                return
            }
            stop() // a local-only hotspot: no good for this
        }
        if (!allowed(context)) {
            done(null)
            return
        }
        val app = context.applicationContext
        var answered = false
        val answer = { result: Map<String, Any>? ->
            if (!answered) {
                answered = true
                details = result
                Log.i(TAG, "direct link: ${result?.let { "${it["ssid"]} at ${it["mhz"]} MHz" } ?: "none"}")
                done(result)
            }
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startDirect(app, tries = 3) { group ->
                when {
                    group != null -> answer(group)
                    directOnly -> answer(null)
                    else -> startHotspot(app, answer)
                }
            }
        } else if (!directOnly) {
            startHotspot(app, answer)
        } else {
            answer(null)
        }
    }

    fun stop() {
        details = null
        reservation?.close()
        reservation = null
        direct?.let { (p2p, channel) ->
            p2p.removeGroup(channel, null)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) channel.close()
        }
        direct = null
    }

    // Wi-Fi Direct.

    /**
     * Starts a Wi-Fi Direct group, trying up to [tries] times: it sometimes
     * fails for a moment (busy with a scan, a group left over), and the
     * fallback is much worse.
     */
    private fun startDirect(context: Context, tries: Int, done: (Map<String, Any>?) -> Unit) {
        startDirectOnce(context) { group ->
            if (group != null || tries <= 1) {
                done(group)
            } else {
                Handler(Looper.getMainLooper()).postDelayed({ startDirect(context, tries - 1, done) }, 700)
            }
        }
    }

    @SuppressLint("MissingPermission") // checked in allowed()
    private fun startDirectOnce(context: Context, done: (Map<String, Any>?) -> Unit) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return done(null)
        val p2p = context.getSystemService(WifiP2pManager::class.java) ?: return done(null)
        val channel = p2p.initialize(context, Looper.getMainLooper(), null) ?: return done(null)
        val main = Handler(Looper.getMainLooper())
        val name = "DIRECT-${random(2)}-Nudge"
        val passphrase = random(16)
        val config = try {
            val builder = WifiP2pConfig.Builder().setNetworkName(name).setPassphrase(passphrase)
            // Two different 5 GHz channels at once is often more than the
            // radio does, so share the phone's own channel when it's on 5 GHz.
            val mhz = wifiMhz(context)
            if (mhz in 5150..5895) {
                builder.setGroupOperatingFrequency(mhz)
            } else {
                builder.setGroupOperatingBand(WifiP2pConfig.GROUP_OWNER_BAND_5GHZ)
            }
            builder.build()
        } catch (e: Exception) {
            Log.i(TAG, "direct link: can't set up Wi-Fi Direct ($e)")
            return done(null)
        }
        val fail = { why: String ->
            Log.i(TAG, "direct link: Wi-Fi Direct didn't start ($why)")
            // Clear whatever group is in the way, for the next try.
            runCatching { p2p.removeGroup(channel, null) }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) channel.close()
            done(null)
        }
        try {
            p2p.createGroup(channel, config, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    direct = p2p to channel
                    // The group takes a moment to form after "success".
                    var tries = 0
                    fun look() {
                        p2p.requestGroupInfo(channel) { group: WifiP2pGroup? ->
                            when {
                                group != null && group.isGroupOwner -> done(
                                    mapOf(
                                        "ssid" to (group.networkName ?: name),
                                        "passphrase" to (group.passphrase ?: passphrase),
                                        "wpa3" to false,
                                        "mhz" to groupMhz(group),
                                    ),
                                )
                                ++tries < 20 -> main.postDelayed(::look, 150)
                                else -> {
                                    direct = null
                                    p2p.removeGroup(channel, null)
                                    fail("no group formed")
                                }
                            }
                        }
                    }
                    look()
                }

                override fun onFailure(reason: Int) = fail(
                    when (reason) {
                        WifiP2pManager.BUSY -> "busy"
                        WifiP2pManager.P2P_UNSUPPORTED -> "unsupported"
                        else -> "error $reason"
                    },
                )
            })
        } catch (e: Exception) {
            fail(e.toString())
        }
    }

    private fun groupMhz(group: WifiP2pGroup): Int =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) group.frequency else 0

    // Local-only hotspot.

    @SuppressLint("MissingPermission") // checked in allowed()
    private fun startHotspot(context: Context, done: (Map<String, Any>?) -> Unit) {
        val wifi = context.getSystemService(Context.WIFI_SERVICE) as WifiManager
        fun callback(onFail: (Int) -> Unit) = object : WifiManager.LocalOnlyHotspotCallback() {
            override fun onStarted(started: WifiManager.LocalOnlyHotspotReservation) {
                reservation = started
                done(hotspotDetails(started))
            }

            override fun onFailed(reason: Int) = onFail(reason)

            override fun onStopped() {
                reservation = null
                details = null
            }
        }
        fun plain() = try {
            wifi.startLocalOnlyHotspot(callback { done(null) }, Handler(Looper.getMainLooper()))
        } catch (e: Exception) {
            // Not allowed right now (permission, location off, tethering on).
            done(null)
        }
        // Android 16+: ask for 5 GHz, on the phone's own channel when it's
        // on 5 GHz. The phone may still pick 2.4 GHz.
        if (Build.VERSION.SDK_INT >= 36) {
            try {
                val mhz = wifiMhz(context)
                val channel = if (mhz in 5150..5895) (mhz - 5000) / 5 else 0
                val config = SoftApConfiguration.Builder()
                    .setChannels(SparseIntArray().apply { put(SoftApConfiguration.BAND_5GHZ, channel) })
                    .build()
                wifi.startLocalOnlyHotspotWithConfiguration(config, context.mainExecutor, callback { plain() })
                return
            } catch (e: Exception) {
                // Not on this phone: the usual way below.
            }
        }
        plain()
    }

    private fun hotspotDetails(r: WifiManager.LocalOnlyHotspotReservation): Map<String, Any>? {
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
                "mhz" to 0,
            )
        }
        @Suppress("DEPRECATION")
        val config = r.wifiConfiguration ?: return null
        @Suppress("DEPRECATION")
        return mapOf(
            "ssid" to config.SSID.removeSurrounding("\""),
            "passphrase" to (config.preSharedKey ?: "").removeSurrounding("\""),
            "wpa3" to false,
            "mhz" to 0,
        )
    }

    /** The frequency this phone's Wi-Fi is on, in MHz, or 0. */
    @Suppress("DEPRECATION")
    private fun wifiMhz(context: Context): Int {
        val wifi = context.getSystemService(Context.WIFI_SERVICE) as WifiManager
        return runCatching { wifi.connectionInfo.frequency }.getOrDefault(0)
    }

    private fun random(length: Int): String {
        val chars = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789"
        val rng = SecureRandom()
        return String(CharArray(length) { chars[rng.nextInt(chars.length)] })
    }
}
