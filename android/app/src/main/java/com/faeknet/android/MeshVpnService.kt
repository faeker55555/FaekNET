package com.faeknet.android

import android.content.Intent
import android.net.VpnService
import android.os.ParcelFileDescriptor
import android.util.Base64
import android.util.Log
import java.io.FileInputStream
import java.io.FileOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Minimal direct-peer transport. It matches the desktop wire format:
 * nonce(12) || ChaCha20-Poly1305(type || sender VIP || raw IPv4 packet).
 * This intentionally has no relay or repository behavior yet.
 */
class MeshVpnService : VpnService() {
    private var tun: ParcelFileDescriptor? = null
    private var socket: DatagramSocket? = null
    @Volatile private var running = false
    private val random = SecureRandom()
    private val workers = mutableListOf<Thread>()

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        stopTransport()
        val vip = intent?.getStringExtra(EXTRA_VIP) ?: "10.66.0.1"
        val port = intent?.getIntExtra(EXTRA_PORT, 54321) ?: 54321
        val psk = intent?.getStringExtra(EXTRA_PSK).orEmpty()
        val peers = parsePeers(intent?.getStringExtra(EXTRA_PEERS).orEmpty())
        val key = runCatching { Base64.decode(psk, Base64.DEFAULT) }.getOrNull()
        if (key == null || key.size != 32 || peers.isEmpty()) {
            Log.e(TAG, "Need a base64 32-byte PSK and at least one peer endpoint")
            stopSelf()
            return START_NOT_STICKY
        }
        startForeground(NOTIFICATION_ID, notification("Direct mesh · ${peers.size} peer(s)"))
        try {
            tun = Builder().setSession("FaekNET").setMtu(1400)
                .addAddress(vip, 24).addRoute("10.66.0.0", 24).establish()
                ?: error("Android refused VPN establishment")
            socket = DatagramSocket(null).apply {
                reuseAddress = true
                bind(InetSocketAddress(port))
                if (!protect(this)) error("Could not protect mesh socket from VPN loop")
                soTimeout = 250
            }
        } catch (e: Exception) {
            Log.e(TAG, "Mesh start failed", e); stopTransport(); stopSelf(); return START_NOT_STICKY
        }
        running = true
        val local = ipv4(vip)
        val cipher = MeshCipher(key)
        workers += Thread { tunToPeers(local, cipher, peers) }.also { it.start() }
        workers += Thread { peersToTun(cipher) }.also { it.start() }
        return START_STICKY
    }

    private fun tunToPeers(local: ByteArray, cipher: MeshCipher, peers: List<Peer>) {
        val input = FileInputStream(tun!!.fileDescriptor); val buf = ByteArray(32767)
        try { while (running) {
            val n = input.read(buf); if (n < 20 || buf[0].toInt() shr 4 != 4) continue
            val dst = buf.copyOfRange(16, 20)
            val targets = if (dst[0].toInt() and 0xff == 10 && dst[1].toInt() and 0xff == 66 && dst[2].toInt() and 0xff == 0 && (dst[3].toInt() and 0xff) != 255)
                peers.filter { it.vip.contentEquals(dst) } else peers
            if (targets.isEmpty()) continue
            val plain = ByteArray(5 + n); plain[0] = TYPE_DATA; System.arraycopy(local, 0, plain, 1, 4); System.arraycopy(buf, 0, plain, 5, n)
            val wire = cipher.seal(plain)
            for (peer in targets) socket?.send(DatagramPacket(wire, wire.size, peer.address))
        } } catch (e: Exception) { if (running) Log.e(TAG, "TUN read failed", e) } finally { input.close() }
    }

    private fun peersToTun(cipher: MeshCipher) {
        val output = FileOutputStream(tun!!.fileDescriptor); val buf = ByteArray(65535)
        try { while (running) {
            val packet = DatagramPacket(buf, buf.size)
            try { socket?.receive(packet) } catch (_: java.net.SocketTimeoutException) { continue }
            val plain = runCatching { cipher.open(packet.data.copyOf(packet.length)) }.getOrNull() ?: continue
            if (plain.size < 5 || plain[0] != TYPE_DATA) continue
            val ip = plain.copyOfRange(5, plain.size)
            if (ip.size >= 20 && ip[0].toInt() shr 4 == 4) output.write(ip)
        } } catch (e: Exception) { if (running) Log.e(TAG, "UDP read failed", e) } finally { output.close() }
    }

    private fun stopTransport() { running = false; workers.forEach { it.interrupt() }; workers.clear(); socket?.close(); socket = null; tun?.close(); tun = null }
    override fun onDestroy() { stopTransport(); super.onDestroy() }

    private fun parsePeers(raw: String): List<Peer> = raw.split(',').mapNotNull { part ->
        val x = part.trim().split('='); if (x.size != 2) return@mapNotNull null
        val vip = ipv4(x[0].trim()); val endpoint = x[1].trim().split(':'); if (vip.size != 4 || endpoint.size != 2) return@mapNotNull null
        val ip = runCatching { java.net.InetAddress.getByName(endpoint[0]) }.getOrNull() ?: return@mapNotNull null
        val port = endpoint[1].toIntOrNull()?.takeIf { it in 1..65535 } ?: return@mapNotNull null
        Peer(vip, InetSocketAddress(ip, port))
    }
    private fun ipv4(text: String): ByteArray = text.split('.').mapNotNull { it.toIntOrNull()?.takeIf { n -> n in 0..255 } }.map { it.toByte() }.toByteArray()
    private fun notification(text: String) = MainActivity.notification(this, text)

    data class Peer(val vip: ByteArray, val address: InetSocketAddress)
    private class MeshCipher(private val keyBytes: ByteArray) {
        private val random = SecureRandom()
        fun seal(plain: ByteArray): ByteArray { val nonce = ByteArray(12); random.nextBytes(nonce); val c = Cipher.getInstance("ChaCha20-Poly1305"); c.init(Cipher.ENCRYPT_MODE, SecretKeySpec(keyBytes, "ChaCha20"), IvParameterSpec(nonce)); return nonce + c.doFinal(plain) }
        fun open(wire: ByteArray): ByteArray { require(wire.size >= 28); val c = Cipher.getInstance("ChaCha20-Poly1305"); c.init(Cipher.DECRYPT_MODE, SecretKeySpec(keyBytes, "ChaCha20"), IvParameterSpec(wire.copyOfRange(0, 12))); return c.doFinal(wire.copyOfRange(12, wire.size)) }
    }
    companion object { const val EXTRA_VIP="vip"; const val EXTRA_PORT="port"; const val EXTRA_PSK="psk"; const val EXTRA_PEERS="peers"; private const val TYPE_DATA: Byte=1; private const val TAG="FaekNET"; private const val NOTIFICATION_ID=4402 }
}
