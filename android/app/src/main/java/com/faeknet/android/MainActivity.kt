package com.faeknet.android

import android.app.Activity
import android.content.Intent
import android.net.VpnService
import android.os.Bundle
import android.graphics.Color
import android.view.ViewGroup
import android.widget.*

/** Minimal direct-peer configuration for the first usable APK. */
class MainActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var vip: EditText
    private lateinit var port: EditText
    private lateinit var psk: EditText
    private lateinit var peers: EditText

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root = LinearLayout(this).apply { orientation=LinearLayout.VERTICAL; setPadding(32,28,32,24); setBackgroundColor(Color.rgb(23,22,22)) }
        root.addView(label("FaekNET Android", 28f, Color.rgb(233,228,223)))
        status = label("Direct mesh · stopped", 14f, Color.rgb(155,145,137)); root.addView(status)
        root.addView(label("Routes 10.66.0.0/24 through the VPN. Other traffic stays on Android's normal network.", 13f, Color.rgb(181,170,162)))
        vip = field(root, "Your virtual IP", "10.66.0.1")
        port = field(root, "Local UDP port", "54321")
        psk = field(root, "Private PSK (base64, 32 bytes)", "", true)
        peers = field(root, "Peers: virtual-ip=public-ip:port, comma separated", "10.66.0.2=203.0.113.25:54321")
        val start = Button(this).apply { text="START MESH VPN"; setOnClickListener { requestVpnPermission() } }
        root.addView(start, LinearLayout.LayoutParams(-1,-2).apply { topMargin=20 })
        val stop = Button(this).apply { text="STOP"; setOnClickListener { stopService(Intent(this@MainActivity, MeshVpnService::class.java)); status.text="Direct mesh · stopped" } }
        root.addView(stop)
        root.addView(label("This first APK supports direct encrypted IPv4 peer traffic for SSH, HTTP and similar applications. Relay routes, repository discovery and NAT roaming are the next bridge layer.", 12f, Color.rgb(155,145,137)))
        setContentView(ScrollView(this).apply { addView(root) })
    }

    private fun label(text: String, size: Float, color: Int) = TextView(this).apply { this.text=text; textSize=size; setTextColor(color); setPadding(0,10,0,10) }
    private fun field(root: LinearLayout, hint: String, value: String, password: Boolean=false): EditText {
        root.addView(label(hint, 12f, Color.rgb(155,145,137)))
        return EditText(this).apply { setText(value); textSize=14f; setTextColor(Color.rgb(233,228,223)); setHintTextColor(Color.rgb(120,112,106)); hint=hint; if(password) inputType=0x81; root.addView(this, ViewGroup.LayoutParams(-1,-2)) }
    }
    private fun requestVpnPermission() {
        val intent=VpnService.prepare(this); if(intent != null) startActivityForResult(intent, REQUEST_VPN) else startMesh()
    }
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) { super.onActivityResult(requestCode,resultCode,data); if(requestCode==REQUEST_VPN && resultCode==RESULT_OK) startMesh() }
    private fun startMesh() {
        startService(Intent(this, MeshVpnService::class.java).apply { putExtra(MeshVpnService.EXTRA_VIP,vip.text.toString().trim()); putExtra(MeshVpnService.EXTRA_PORT,port.text.toString().toIntOrNull() ?: 54321); putExtra(MeshVpnService.EXTRA_PSK,psk.text.toString().trim()); putExtra(MeshVpnService.EXTRA_PEERS,peers.text.toString().trim()) })
        status.text="Direct mesh · starting"
    }
    companion object { private const val REQUEST_VPN=7001; fun notification(service: android.content.Context, text: String): android.app.Notification { val id="faeknet-vpn"; val manager=service.getSystemService(android.app.NotificationManager::class.java); if(android.os.Build.VERSION.SDK_INT>=26) manager.createNotificationChannel(android.app.NotificationChannel(id,"FaekNET VPN",android.app.NotificationManager.IMPORTANCE_LOW)); return if(android.os.Build.VERSION.SDK_INT>=26) android.app.Notification.Builder(service,id).setContentTitle("FaekNET").setContentText(text).setSmallIcon(android.R.drawable.stat_sys_data_connected).setOngoing(true).build() else { @Suppress("DEPRECATION") val b=android.app.Notification.Builder(service); b.setContentTitle("FaekNET").setContentText(text).setSmallIcon(android.R.drawable.stat_sys_data_connected).setOngoing(true).build() } } }
}
