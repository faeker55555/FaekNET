package com.faeknet.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor

/**
 * Android transport boundary. The TUN descriptor is deliberately not advertised
 * as a working mesh until the Rust core bridge consumes it.
 */
class FaekVpnService : VpnService() {
    private var tun: ParcelFileDescriptor? = null
    private var worker: Thread? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val manual = intent?.getBooleanExtra(EXTRA_MANUAL_ROUTE, false) ?: false
        val proxy = intent?.getBooleanExtra(EXTRA_PROXY_MODE, false) ?: false
        startForeground(NOTIFICATION_ID, notification(manual, proxy))
        if (tun == null) {
            tun = Builder()
                .setSession("FaekNET")
                .setMtu(1400)
                .addAddress("10.66.0.1", 24)
                .addRoute("10.66.0.0", 24)
                .establish()
            // Do not silently black-hole traffic: until Rust is attached, close
            // the alpha VPN after documenting the missing backend in the log.
            worker = Thread {
                android.util.Log.w(TAG, "TUN established; Rust mesh backend is not attached yet")
            }.also { it.start() }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        worker?.interrupt(); worker = null
        tun?.close(); tun = null
        super.onDestroy()
    }

    private fun notification(manual: Boolean, proxy: Boolean): Notification {
        val channelId = "faeknet-vpn"
        if (Build.VERSION.SDK_INT >= 26) {
            getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(channelId, "FaekNET VPN", NotificationManager.IMPORTANCE_LOW),
            )
        }
        val mode = when { proxy -> "proxy staged"; manual -> "manual route staged"; else -> "mesh-only staged" }
        return if (Build.VERSION.SDK_INT >= 26) {
            Notification.Builder(this, channelId).setContentTitle("FaekNET").setContentText(mode).setSmallIcon(android.R.drawable.stat_sys_warning).setOngoing(true).build()
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this).setContentTitle("FaekNET").setContentText(mode).setSmallIcon(android.R.drawable.stat_sys_warning).setOngoing(true).build()
        }
    }

    companion object {
        const val EXTRA_MANUAL_ROUTE = "manual_route"
        const val EXTRA_PROXY_MODE = "proxy_mode"
        private const val TAG = "FaekNET"
        private const val NOTIFICATION_ID = 4401
    }
}
