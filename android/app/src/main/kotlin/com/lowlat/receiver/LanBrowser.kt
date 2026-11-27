package com.lowlat.receiver

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.lowlat.protocol.Beacon
import com.lowlat.protocol.Wire
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.charset.StandardCharsets
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Discovers EcConn Receivers on the LAN using both NSD (mDNS) and UDP broadcast
 * beacons on port 9200, mirroring `app-host::Discovery`.
 */
class LanBrowser(
    private val context: Context,
    private val selfDeviceId: String,
) {
    data class DiscoveredReceiver(
        val beacon: Beacon,
        val address: InetAddress,
        val source: String,
        val lastSeenMs: Long,
    )

    companion object {
        private const val TAG = "LanBrowser"
        private const val STALE_MS = 15_000L
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val running = AtomicBoolean(false)
    private val devices = ConcurrentHashMap<String, DiscoveredReceiver>()

    private var beaconSocket: DatagramSocket? = null
    private var beaconThread: Thread? = null
    private var nsdManager: NsdManager? = null
    private var discoveryListener: NsdManager.DiscoveryListener? = null

    var onDevicesChanged: ((List<DiscoveredReceiver>) -> Unit)? = null

    private val pruneRunnable = object : Runnable {
        override fun run() {
            if (!running.get()) return
            val now = System.currentTimeMillis()
            var removed = false
            val it = devices.entries.iterator()
            while (it.hasNext()) {
                if (now - it.next().value.lastSeenMs > STALE_MS) {
                    it.remove()
                    removed = true
                }
            }
            if (removed) {
                notifyListeners()
            }
            mainHandler.postDelayed(this, 2000L)
        }
    }

    fun start() {
        if (running.getAndSet(true)) return
        devices.clear()
        startBeaconListener()
        startMdnsDiscovery()
        mainHandler.postDelayed(pruneRunnable, 2000L)
    }

    fun stop() {
        if (!running.getAndSet(false)) return
        mainHandler.removeCallbacks(pruneRunnable)
        try {
            beaconSocket?.close()
        } catch (_: Throwable) {}
        beaconSocket = null
        beaconThread = null

        val listener = discoveryListener
        if (listener != null) {
            try {
                nsdManager?.stopServiceDiscovery(listener)
            } catch (_: Throwable) {}
        }
        discoveryListener = null
        devices.clear()
    }

    fun snapshot(): List<DiscoveredReceiver> {
        val now = System.currentTimeMillis()
        return devices.values
            .filter { now - it.lastSeenMs <= STALE_MS }
            .sortedBy { it.beacon.name.lowercase() }
    }

    private fun startBeaconListener() {
        val thread = Thread({
            try {
                val sock = DatagramSocket(Wire.BEACON_PORT).apply {
                    reuseAddress = true
                    soTimeout = 500
                }
                beaconSocket = sock
                val buf = ByteArray(1024)
                val packet = DatagramPacket(buf, buf.size)
                while (running.get()) {
                    try {
                        packet.length = buf.size
                        sock.receive(packet)
                        val text = String(packet.data, packet.offset, packet.length, StandardCharsets.UTF_8)
                        val beacon = Beacon.parse(text) ?: continue
                        if (beacon.id == selfDeviceId) continue
                        val addr = packet.address ?: continue
                        devices[beacon.id] = DiscoveredReceiver(
                            beacon = beacon,
                            address = addr,
                            source = "beacon",
                            lastSeenMs = System.currentTimeMillis(),
                        )
                        notifyListeners()
                    } catch (_: java.net.SocketTimeoutException) {
                        // loop
                    } catch (e: Exception) {
                        if (running.get()) Log.w(TAG, "Beacon recv error: ${e.message}")
                    }
                }
            } catch (e: Exception) {
                Log.w(TAG, "Could not bind beacon port ${Wire.BEACON_PORT}: ${e.message}")
            }
        }, "EcConnLanBrowser")
        beaconThread = thread
        thread.start()
    }

    private fun startMdnsDiscovery() {
        try {
            val mgr = context.getSystemService(Context.NSD_SERVICE) as? NsdManager ?: return
            nsdManager = mgr
            val listener = object : NsdManager.DiscoveryListener {
                override fun onStartDiscoveryFailed(serviceType: String?, errorCode: Int) {
                    Log.w(TAG, "NSD start failed: $errorCode")
                }
                override fun onStopDiscoveryFailed(serviceType: String?, errorCode: Int) {}
                override fun onDiscoveryStarted(serviceType: String?) {
                    Log.i(TAG, "NSD discovery started for $serviceType")
                }
                override fun onDiscoveryStopped(serviceType: String?) {}
                override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                    @Suppress("DEPRECATION")
                    mgr.resolveService(serviceInfo, object : NsdManager.ResolveListener {
                        override fun onResolveFailed(si: NsdServiceInfo?, errorCode: Int) {}
                        override fun onServiceResolved(resolved: NsdServiceInfo) {
                            val attrs = resolved.attributes
                            val idBytes = attrs["id"] ?: return
                            val id = String(idBytes, StandardCharsets.UTF_8)
                            if (id == selfDeviceId) return
                            val name = attrs["name"]?.let { String(it, StandardCharsets.UTF_8) }
                                ?: resolved.serviceName
                            val platform = attrs["platform"]?.let { String(it, StandardCharsets.UTF_8) }
                                ?: "unknown"
                            val w = attrs["w"]?.let { String(it, StandardCharsets.UTF_8).toIntOrNull() } ?: 1920
                            val h = attrs["h"]?.let { String(it, StandardCharsets.UTF_8).toIntOrNull() } ?: 1080
                            val orient = attrs["orient"]?.let { String(it, StandardCharsets.UTF_8).toIntOrNull() } ?: 0
                            val pairPort = attrs["pair"]?.let { String(it, StandardCharsets.UTF_8).toIntOrNull() }
                                ?: Wire.PAIR_PORT
                            val host = resolved.host ?: return
                            val beacon = Beacon(
                                id = id,
                                name = name,
                                platform = platform,
                                width = w,
                                height = h,
                                orientation = orient,
                                videoPort = resolved.port.takeIf { it > 0 } ?: Wire.VIDEO_PORT,
                                pairPort = pairPort,
                            )
                            devices[id] = DiscoveredReceiver(
                                beacon = beacon,
                                address = host,
                                source = "mdns",
                                lastSeenMs = System.currentTimeMillis(),
                            )
                            notifyListeners()
                        }
                    })
                }
                override fun onServiceLost(serviceInfo: NsdServiceInfo?) {}
            }
            discoveryListener = listener
            mgr.discoverServices(Wire.MDNS_SERVICE, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: Exception) {
            Log.w(TAG, "NSD discovery unavailable: ${e.message}")
        }
    }

    private fun notifyListeners() {
        val snap = snapshot()
        mainHandler.post {
            onDevicesChanged?.invoke(snap)
        }
    }
}
