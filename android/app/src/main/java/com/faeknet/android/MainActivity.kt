package com.faeknet.android

import android.app.Activity
import android.content.Intent
import android.net.VpnService
import android.os.Bundle
import android.view.Gravity
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Switch
import android.widget.TextView

/** First Android shell. Mesh packets are not handled until the Rust bridge is attached. */
class MainActivity : Activity() {
    private lateinit var status: TextView
    private var manualRoute = false
    private var proxyMode = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(32, 32, 32, 24)
            setBackgroundColor(0xFF171616.toInt())
        }
        val title = TextView(this).apply {
            text = "FaekNET"
            textSize = 28f
            setTextColor(0xFFE9E4DF.toInt())
        }
        root.addView(title)
        status = TextView(this).apply {
            text = "Android VPN shell · stopped"
            textSize = 14f
            setTextColor(0xFF9B9189.toInt())
            setPadding(0, 20, 0, 20)
        }
        root.addView(status)

        val meshOnly = TextView(this).apply {
            text = "Mesh-only routing\n10.66.0.0/24 → FaekNET VPN\nOther traffic → Android normally"
            textSize = 15f
            setTextColor(0xFFB5AAA2.toInt())
            setPadding(0, 12, 0, 20)
        }
        root.addView(meshOnly)
        addSwitch(root, "Manual route selection", "Hold the selected authenticated route", false) { manualRoute = it }
        addSwitch(root, "Proxy non-mesh traffic", "TCP proxy plumbing is not connected yet", false) { proxyMode = it }

        val start = Button(this).apply {
            text = "START VPN"
            setOnClickListener { requestVpnPermission() }
        }
        root.addView(start, LinearLayout.LayoutParams(-1, -2).apply { topMargin = 24 })
        val note = TextView(this).apply {
            text = "Alpha shell: the VPN permission and split route are implemented first. Rust mesh transport, relay forwarding, repository sync, and proxy forwarding are staged behind the native bridge."
            textSize = 12f
            setTextColor(0xFF9B9189.toInt())
            setPadding(0, 24, 0, 0)
        }
        root.addView(note)
        setContentView(ScrollView(this).apply { addView(root) })
    }

    private fun addSwitch(parent: LinearLayout, label: String, detail: String, checked: Boolean, changed: (Boolean) -> Unit) {
        val row = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(0, 10, 0, 4) }
        val toggle = Switch(this).apply {
            text = label
            textSize = 15f
            setTextColor(0xFFE9E4DF.toInt())
            isChecked = checked
            setOnCheckedChangeListener { _, value -> changed(value) }
        }
        row.addView(toggle)
        row.addView(TextView(this).apply { text = detail; textSize = 12f; setTextColor(0xFF9B9189.toInt()) })
        parent.addView(row)
    }

    private fun requestVpnPermission() {
        val intent = VpnService.prepare(this)
        if (intent != null) startActivityForResult(intent, REQUEST_VPN)
        else onActivityResult(REQUEST_VPN, RESULT_OK, null)
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_VPN || resultCode != RESULT_OK) return
        startService(Intent(this, FaekVpnService::class.java).apply {
            putExtra(FaekVpnService.EXTRA_MANUAL_ROUTE, manualRoute)
            putExtra(FaekVpnService.EXTRA_PROXY_MODE, proxyMode)
        })
        status.text = "Android VPN · starting"
    }

    companion object { private const val REQUEST_VPN = 7001 }
}
