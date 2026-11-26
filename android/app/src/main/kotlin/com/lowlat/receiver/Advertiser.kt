package com.lowlat.receiver

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import com.lowlat.protocol.Beacon
import com.lowlat.protocol.Wire
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.charset.StandardCharsets
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

/**
 * Handles LAN advertisement for the receiver:
 * 1. mDNS / NSD service `_lowlat._udp.` on port 9100 with TXT attributes.
 * 2. UDP fallback broadcast beacon on port 9200 every 2 seconds.
 */
class Advertiser(private val context: Context) {

    companion object {
        private const val TAG = "Advertiser"
        private const val BROADCAST_INTERVAL_SEC = 2L
    }

    private val nsdManager = context.getSystemService(Context.NSD_SERVICE) as? NsdManager
    private var registrationListener: NsdManager.RegistrationListener? = null
    private var isNsdRegistered = false

    private var broadcastExecutor: ScheduledExecutorService? = null
    private var broadcastSocket: DatagramSocket? = null

    private var deviceId = DisplayInfo.getDeviceId(context)
    private var deviceName = DisplayInfo.getDeviceName()
    private var width = 1920
    private var height = 1080
    private var orientation = 0

    @Synchronized
    fun start(initialWidth: Int, initialHeight: Int, initialOrientation: Int) {
        width = initialWidth
        height = initialHeight
        orientation = initialOrientation

        startNsd()
        startUdpBeacon()
    }

    @Synchronized
    fun updateDisplay(newWidth: Int, newHeight: Int, newOrientation: Int) {
        if (width == newWidth && height == newHeight && orientation == newOrientation) return
        width = newWidth
        height = newHeight
        orientation = newOrientation

        // Restart NSD to update TXT records
        stopNsd()
        startNsd()
    }

    private fun startNsd() {
        if (nsdManager == null) {
            Log.w(TAG, "NsdManager not available on this device")
            return
        }

        try {
            val serviceInfo = NsdServiceInfo().apply {
                serviceName = deviceName
                serviceType = Wire.MDNS_SERVICE
                port = Wire.VIDEO_PORT

                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                    setAttribute("id", deviceId)
                    setAttribute("name", deviceName)
                    setAttribute("platform", "android")
                    setAttribute("w", width.toString())
                    setAttribute("h", height.toString())
                    setAttribute("orient", orientation.toString())
                    setAttribute("pair", Wire.PAIR_PORT.toString())
                    setAttribute("video", Wire.VIDEO_PORT.toString())
                }
            }

            val listener = object : NsdManager.RegistrationListener {
                override fun onServiceRegistered(info: NsdServiceInfo) {
                    isNsdRegistered = true
                    Log.i(TAG, "NSD service registered: ${info.serviceName}")
                }

                override fun onRegistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                    isNsdRegistered = false
                    Log.e(TAG, "NSD registration failed: error code $errorCode")
                }

                override fun onServiceUnregistered(info: NsdServiceInfo) {
                    isNsdRegistered = false
                    Log.i(TAG, "NSD service unregistered: ${info.serviceName}")
                }

                override fun onUnregistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                    Log.e(TAG, "NSD unregistration failed: error code $errorCode")
                }
            }

            registrationListener = listener
            nsdManager.registerService(serviceInfo, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to register NSD service", e)
        }
    }

    private fun stopNsd() {
        val listener = registrationListener ?: return
        if (isNsdRegistered && nsdManager != null) {
            try {
                nsdManager.unregisterService(listener)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to unregister NSD service: ${e.message}")
            }
        }
        registrationListener = null
        isNsdRegistered = false
    }

    private fun startUdpBeacon() {
        if (broadcastExecutor != null) return

        try {
            val socket = DatagramSocket()
            socket.broadcast = true
            broadcastSocket = socket
        } catch (e: Exception) {
            Log.e(TAG, "Failed to create UDP broadcast socket", e)
            return
        }

        val executor = Executors.newSingleThreadScheduledExecutor()
        broadcastExecutor = executor
        executor.scheduleWithFixedDelay({
            sendBeacon()
        }, 0L, BROADCAST_INTERVAL_SEC, TimeUnit.SECONDS)
    }

    private fun sendBeacon() {
        val socket = broadcastSocket ?: return
        try {
            val beacon = Beacon(
                id = deviceId,
                name = deviceName,
                platform = "android",
                width = width,
                height = height,
                orientation = orientation,
                videoPort = Wire.VIDEO_PORT,
                pairPort = Wire.PAIR_PORT,
            )
            val text = beacon.encode() + "\n"
            val bytes = text.toByteArray(StandardCharsets.UTF_8)

            // Send to generic broadcast address
            val broadcastAddr = InetAddress.getByName("255.255.255.255")
            val packet = DatagramPacket(bytes, bytes.size, broadcastAddr, Wire.BEACON_PORT)
            socket.send(packet)
        } catch (e: Exception) {
            Log.w(TAG, "Beacon broadcast tick failed: ${e.message}")
        }
    }

    private fun stopUdpBeacon() {
        try {
            broadcastExecutor?.shutdownNow()
        } catch (ignored: Throwable) {}
        broadcastExecutor = null

        try {
            broadcastSocket?.close()
        } catch (ignored: Throwable) {}
        broadcastSocket = null
    }

    @Synchronized
    fun stop() {
        stopNsd()
        stopUdpBeacon()
    }
}
